//! The completion engine: a pure function of `(source, offset, context)`.
//!
//! It combines four sources, in priority order:
//!   1. the program's own scope (declarations, params, imports),
//!   2. host-exposed functions and registered modules (from the context),
//!   3. the built-in global catalog,
//!   4. language keywords.
//!
//! Member completion (`recv.pref|`) resolves the receiver statically: known
//! globals use the catalog, `import * as ns` resolves to a module's exports,
//! and variables with literal initializers reveal their keys/element type.
//! Because it works on the error-tolerant parse, it behaves while code is still
//! being typed and does not yet run.

use crate::lexer::Lexer;
use crate::parser::{Parser, Statement};

use super::catalog::{self, ProtoKind};
use super::scope::{self, InitShape, Scope};
use super::{
    AnalysisContext, Completion, CompletionKind, Document, GlobalInfo, Type,
    playground_completion_module_prefix, playground_module_name, playground_module_specifier,
};

/// Compute completion candidates at a byte offset in the source.
pub fn complete(source: &str, offset: usize, ctx: &AnalysisContext) -> Vec<Completion> {
    // Snap the offset to a char boundary to avoid slicing mid-codepoint.
    let offset = snap_boundary(source, offset.min(source.len()));
    let before = &source[..offset];

    let stmts = parse_lenient(source);
    let scope = scope::collect(&stmts, &ctx.runtime_handlers);
    let globals = ctx.resolved_globals();
    let document = Document::parse_with_context_and_runtime_and_globals(
        source,
        &std::collections::HashMap::new(),
        &ctx.host_functions(),
        &ctx.runtime_handlers,
        &std::collections::HashMap::new(),
        &globals,
    );

    match analyze_trigger(before) {
        Trigger::Member { receiver, prefix } => {
            complete_member(&receiver, &prefix, &scope, ctx, &document)
        }
        Trigger::Ident { prefix } => complete_ident(&prefix, before, &scope, ctx),
    }
}

/// Return the member receiver and identifier prefix at a cursor position.
/// The WASM host uses this to merge runtime members with static candidates.
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub fn member_trigger(source: &str, offset: usize) -> Option<(String, String)> {
    let offset = snap_boundary(source, offset.min(source.len()));
    match_member(&source[..offset])
}

/// Parse without ever failing: the parser skips what it cannot understand, so
/// this always yields the best-effort AST for the currently-typed source.
fn parse_lenient(source: &str) -> Vec<Statement> {
    let toks = Lexer::new(source).tokenize_with_spans();
    let mut parser = Parser::new_with_spans(toks);
    parser.parse()
}

/// Move `offset` back to the nearest UTF-8 char boundary.
fn snap_boundary(s: &str, mut offset: usize) -> usize {
    while offset > 0 && !s.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

// ---- trigger detection ---------------------------------------------------

enum Trigger {
    Member { receiver: String, prefix: String },
    Ident { prefix: String },
}

/// Classify the text before the caret as a member or identifier completion.
fn analyze_trigger(before: &str) -> Trigger {
    if let Some((recv, prefix)) = match_member(before) {
        Trigger::Member {
            receiver: recv,
            prefix,
        }
    } else {
        let prefix = before
            .rsplit_once(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default();
        Trigger::Ident { prefix }
    }
}

/// Match `receiver.prefix` at the end of `before`, covering dotted chains,
/// string-literal receivers, simple array-literal receivers, and
/// `<module namespace prefix><module>.` namespace receivers.
fn match_member(before: &str) -> Option<(String, String)> {
    // Playground namespace receiver first,
    // before generic dotted chains.
    if let Some((recv, prefix)) = playground_member(before) {
        return Some((recv, prefix));
    }
    // Dotted identifier chain: Math.fl / user.addr.va
    if let Some((recv, prefix)) = rsplit_member(before, |c| {
        c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.'
    }) && (recv.contains('.') || is_ident_start(recv.chars().next()))
    {
        return Some((recv, prefix));
    }
    // String literal receiver: "abc".to / 'x'.
    if let Some((recv, prefix)) = literal_receiver(before, '"') {
        return Some((recv, prefix));
    }
    if let Some((recv, prefix)) = literal_receiver(before, '\'') {
        return Some((recv, prefix));
    }
    // Array literal receiver: [1, 2].ma
    if let Some(prefix) = trailing_ident(before) {
        let rest = &before[..before.len() - prefix.len()];
        if rest.ends_with("].")
            && let Some(open) = rest[..rest.len() - 1].rfind('[')
        {
            let recv = rest[open..rest.len() - 1].to_string(); // includes the `]`
            return Some((recv, prefix));
        }
    }
    None
}

fn is_ident_start(c: Option<char>) -> bool {
    matches!(c, Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$')
}

/// Split a trailing `receiver.prefix` where receiver chars satisfy `recv_ok`.
fn rsplit_member(before: &str, recv_ok: impl Fn(char) -> bool) -> Option<(String, String)> {
    let prefix = trailing_ident(before)?;
    let rest = &before[..before.len() - prefix.len()];
    let rest = rest.strip_suffix('.')?;
    let start = rest
        .char_indices()
        .rev()
        .take_while(|&(_, c)| recv_ok(c))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(rest.len());
    let recv = rest[start..].to_string();
    if recv.is_empty() {
        None
    } else {
        Some((recv, prefix))
    }
}

/// The trailing identifier (possibly empty) at the end of `before`.
fn trailing_ident(before: &str) -> Option<String> {
    let rest = before
        .rsplit_once(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .map(|(_, r)| r)
        .unwrap_or(before);
    Some(rest.to_string())
}

/// Match a quoted-string receiver ending just before `.<prefix>`.
fn literal_receiver(before: &str, quote: char) -> Option<(String, String)> {
    let prefix = trailing_ident(before)?;
    let rest = &before[..before.len() - prefix.len()];
    let rest = rest.strip_suffix('.')?;
    if !rest.ends_with(quote) {
        return None;
    }
    // Find the matching opening quote.
    let body = &rest[..rest.len() - 1];
    let open = body.rfind(quote)?;
    let recv = rest[open..].to_string(); // e.g. `"abc"`
    Some((recv, prefix))
}

/// Match a trailing playground-namespace module member pattern.
fn playground_member(before: &str) -> Option<(String, String)> {
    let dot_pos = before.rfind('.')?;
    let recv = &before[..dot_pos];
    let prefix = &before[dot_pos + 1..];
    if playground_module_name(recv).is_some() {
        Some((recv.to_string(), prefix.to_string()))
    } else {
        None
    }
}

// ---- identifier completion -----------------------------------------------

fn complete_ident(
    prefix: &str,
    before: &str,
    scope: &Scope,
    ctx: &AnalysisContext,
) -> Vec<Completion> {
    let mut out: Vec<Completion> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |label: &str, kind: CompletionKind, detail: Option<&str>| {
        if label.starts_with(prefix) && seen.insert(label.to_string()) {
            out.push(Completion {
                label: label.to_string(),
                kind,
                detail: detail.map(|d| d.to_string()),
            });
        }
    };

    // 1. User declarations (highest priority).
    let mut user: Vec<_> = scope.decls.iter().collect();
    user.sort_by(|a, b| a.name.cmp(&b.name));
    for d in user {
        let detail = match d.kind {
            CompletionKind::Function => Some("function"),
            CompletionKind::Class => Some("class"),
            CompletionKind::Module => Some("namespace import"),
            _ => None,
        };
        add(&d.name, d.kind, detail);
    }

    // 2. Custom metadata, in the service-wide precedence order. `add` keeps the
    //    first entry for a name, so a live runtime declaration wins over the
    //    static manifest and legacy host functions sit between the two generic
    //    layers rather than below both. Host functions keep their own
    //    completion kind and signature detail.
    let mut runtime_globals: Vec<_> = ctx.runtime_globals.values().collect();
    runtime_globals.sort_by(|a, b| a.name.cmp(&b.name));
    for global in runtime_globals {
        let detail = global_detail(global);
        add(&global.name, CompletionKind::Global, detail.as_deref());
    }
    let mut exposed = ctx.exposed_functions.clone();
    exposed.sort_by(|a, b| a.name.cmp(&b.name));
    for function in &exposed {
        let detail = function.signature();
        add(&function.name, CompletionKind::ExposedFn, Some(&detail));
    }
    let mut manifest_globals: Vec<_> = ctx.manifest_globals.values().collect();
    manifest_globals.sort_by(|a, b| a.name.cmp(&b.name));
    for global in manifest_globals {
        let detail = global_detail(global);
        add(&global.name, CompletionKind::Global, detail.as_deref());
    }
    let mut manifest_functions = ctx.manifest_functions.clone();
    manifest_functions.sort_by(|a, b| a.name.cmp(&b.name));
    for function in &manifest_functions {
        let detail = function.signature();
        add(&function.name, CompletionKind::ExposedFn, Some(&detail));
    }

    // 3. Modules: registered in the context, plus any imported in the source.
    let mut modules: Vec<String> = ctx.modules.iter().map(|m| m.name.clone()).collect();
    for m in &scope.modules {
        if !modules.contains(m) {
            modules.push(m.clone());
        }
    }
    modules.sort();
    for name in &modules {
        add(name, CompletionKind::Module, Some("module"));
    }

    // 4. Built-in globals.
    for g in catalog::GLOBALS {
        add(g, CompletionKind::Global, None);
    }

    // 5. Keywords (lowest priority).
    for k in catalog::KEYWORDS {
        add(k, CompletionKind::Keyword, None);
    }

    // 6. Playground namespace: offer names when the user is typing a module
    //    specifier with the configured namespace prefix.
    if let Some(module_prefix) = playground_completion_module_prefix(before) {
        let mut playgrounds: Vec<Completion> = Vec::new();
        for ctx_mod in &ctx.modules {
            if ctx_mod.name.starts_with(module_prefix) {
                playgrounds.push(Completion {
                    label: playground_module_specifier(&ctx_mod.name),
                    kind: CompletionKind::Module,
                    detail: Some("playground module".to_string()),
                });
            }
        }
        playgrounds.sort_by(|a, b| a.label.cmp(&b.label));
        for c in playgrounds {
            if seen.insert(c.label.clone()) {
                out.push(c);
            }
        }
    }

    out
}

// ---- member completion ---------------------------------------------------

fn complete_member(
    receiver: &str,
    prefix: &str,
    scope: &Scope,
    ctx: &AnalysisContext,
    document: &Document,
) -> Vec<Completion> {
    let mut out: Vec<Completion> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |label: &str, kind: CompletionKind, detail: Option<String>| {
        if is_member_name(label) && label.starts_with(prefix) && seen.insert(label.to_string()) {
            out.push(Completion {
                label: label.to_string(),
                kind,
                detail,
            });
        }
    };

    let head = receiver.split('.').next().unwrap_or(receiver);

    // The same resolver is used by hover. This is what makes local source
    // bindings shadow runtime/manifest/builtin globals and lets arbitrary
    // metadata resolve through a dotted receiver path.
    let receiver_is_alias = matches!(receiver, "window" | "globalThis" | "self");
    if scope.module_for_namespace(head).is_none()
        && let Some(resolved) = document.resolve_path(receiver)
        && (!receiver_is_alias || document.has_binding(receiver))
    {
        add_runtime_members(&resolved.ty, &mut add);
        return filter_sorted(out);
    }

    // The VM exposes these aliases as views of its global environment. Include
    // both catalog globals and live host functions so window completion works
    // even when the user has not yet executed the file.
    if matches!(receiver, "window" | "globalThis" | "self") {
        // Same precedence order as bare-identifier completion, so `foo` and
        // `globalThis.foo` never disagree about which layer defines `foo`.
        let mut runtime_globals: Vec<_> = ctx.runtime_globals.values().collect();
        runtime_globals.sort_by(|a, b| a.name.cmp(&b.name));
        for global in runtime_globals {
            add(&global.name, CompletionKind::Global, global_detail(global));
        }
        for function in &ctx.exposed_functions {
            add(
                &function.name,
                CompletionKind::ExposedFn,
                Some(function.signature()),
            );
        }
        let mut manifest_globals: Vec<_> = ctx.manifest_globals.values().collect();
        manifest_globals.sort_by(|a, b| a.name.cmp(&b.name));
        for global in manifest_globals {
            add(&global.name, CompletionKind::Global, global_detail(global));
        }
        for function in &ctx.manifest_functions {
            add(
                &function.name,
                CompletionKind::ExposedFn,
                Some(function.signature()),
            );
        }
        for global in catalog::GLOBALS {
            if !matches!(*global, "window" | "globalThis" | "self") {
                add(global, CompletionKind::Global, None);
            }
        }
        return filter_sorted(out);
    }

    // Resolve a catalog global through a global-object alias, such as
    // window.ipc. or globalThis.Math.
    if let Some(global_name) = receiver
        .strip_prefix("window.")
        .or_else(|| receiver.strip_prefix("globalThis."))
        .or_else(|| receiver.strip_prefix("self."))
        && !global_name.contains('.')
        && let Some(members) = catalog::builtin_members(global_name)
    {
        for member in members {
            add(
                member,
                member_kind(member),
                catalog_member_detail(global_name, member),
            );
        }
        return filter_sorted(out);
    }

    // Configured playground namespace → the module's exports.
    if let Some(module_name) = playground_module_name(head)
        && let Some(info) = ctx.modules.iter().find(|m| m.name == module_name)
    {
        let mut exports = info.exports.clone();
        exports.sort();
        for e in &exports {
            add(e, CompletionKind::Property, None);
        }
        return filter_sorted(out);
    }

    // import * as ns  →  the module's exports.
    if let Some(module) = scope.module_for_namespace(head)
        && let Some(info) = ctx.modules.iter().find(|m| m.name == module)
    {
        let mut exports = info.exports.clone();
        exports.sort();
        for e in &exports {
            add(e, CompletionKind::Property, None);
        }
        return filter_sorted(out);
    }

    // Named built-in global (Math, JSON, console, …).
    if let Some(members) = catalog::builtin_members(receiver) {
        for m in members {
            add(m, member_kind(m), catalog_member_detail(receiver, m));
        }
        return filter_sorted(out);
    }

    // Literal receivers.
    if receiver.ends_with(']') {
        for m in catalog::prototype_members(ProtoKind::Array) {
            add(m, CompletionKind::Method, None);
        }
        return filter_sorted(out);
    }
    if receiver.ends_with('"') || receiver.ends_with('\'') {
        for m in catalog::prototype_members(ProtoKind::String) {
            add(m, CompletionKind::Method, None);
        }
        return filter_sorted(out);
    }

    // Variable with a known literal shape.
    if let Some(ty) = runtime_receiver_type(receiver, &scope.runtime_bindings) {
        add_runtime_members(&ty, &mut add);
    }

    if let Some(decl) = scope.find(head) {
        match &decl.shape {
            Some(InitShape::Array) => {
                for m in catalog::prototype_members(ProtoKind::Array) {
                    add(m, CompletionKind::Method, None);
                }
            }
            Some(InitShape::Object(keys)) => {
                let mut keys = keys.clone();
                keys.sort();
                for k in &keys {
                    add(k, CompletionKind::Property, None);
                }
            }
            None => {}
        }
    }

    filter_sorted(out)
}

fn runtime_receiver_type(
    receiver: &str,
    bindings: &std::collections::HashMap<String, super::Type>,
) -> Option<super::Type> {
    let mut segments = receiver.split('.');
    let mut ty = bindings.get(segments.next()?)?.clone();
    for segment in segments {
        ty = ty.property(segment);
    }
    Some(ty)
}

fn add_runtime_members(ty: &Type, add: &mut impl FnMut(&str, CompletionKind, Option<String>)) {
    match ty {
        super::Type::Object(fields) => {
            for (name, field) in fields {
                let detail = type_detail(field);
                let kind = if matches!(field, Type::Function { .. }) {
                    CompletionKind::Method
                } else {
                    CompletionKind::Property
                };
                add(name, kind, detail);
            }
        }
        super::Type::Array(_) => {
            for name in catalog::prototype_members(ProtoKind::Array) {
                add(name, CompletionKind::Method, None);
            }
        }
        super::Type::String => {
            for name in catalog::prototype_members(ProtoKind::String) {
                add(name, CompletionKind::Method, None);
            }
        }
        super::Type::Number => {
            for name in catalog::prototype_members(ProtoKind::Number) {
                add(name, CompletionKind::Method, None);
            }
        }
        super::Type::Promise(_) => {
            for name in catalog::prototype_members(ProtoKind::Promise) {
                add(name, CompletionKind::Method, None);
            }
        }
        super::Type::NativeObject(receiver) => {
            for name in catalog::builtin_members(receiver).into_iter().flatten() {
                add(
                    name,
                    member_kind(name),
                    catalog_member_detail(receiver, name),
                );
            }
        }
        _ => {}
    }
}

fn type_detail(ty: &Type) -> Option<String> {
    if matches!(ty, Type::Function { .. }) {
        Some(ty.display_compact())
    } else {
        None
    }
}

fn global_detail(global: &GlobalInfo) -> Option<String> {
    let ty = Type::from_shape(&global.shape);
    Some(ty.display_compact())
}

fn catalog_member_detail(receiver: &str, member: &str) -> Option<String> {
    catalog::builtin_member_type(receiver, member)
        .map(|builtin| Type::from_builtin(builtin).display_compact())
}

fn is_member_name(label: &str) -> bool {
    !label.is_empty()
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// ALL_CAPS built-ins (Math.PI, Symbol.iterator aside) are constants.
fn member_kind(label: &str) -> CompletionKind {
    let is_const = label
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && label
            .chars()
            .next()
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(false);
    if is_const {
        CompletionKind::Property
    } else {
        CompletionKind::Method
    }
}

fn filter_sorted(mut out: Vec<Completion>) -> Vec<Completion> {
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}
