//! Static semantics shared by cached parsing, eval and compilation.
use super::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

#[derive(Clone, Copy, Default)]
enum FunctionKind {
    #[default]
    Ordinary,
    Arrow,
    Async,
    AsyncArrow,
    Generator,
    AsyncGenerator,
}

impl FunctionKind {
    fn of(arrow: bool, asynchronous: bool, generator: bool) -> Self {
        match (arrow, asynchronous, generator) {
            (true, true, _) => Self::AsyncArrow,
            (true, false, _) => Self::Arrow,
            (false, true, true) => Self::AsyncGenerator,
            (false, true, false) => Self::Async,
            (false, false, true) => Self::Generator,
            _ => Self::Ordinary,
        }
    }
    fn arrow(self) -> bool {
        matches!(self, Self::Arrow | Self::AsyncArrow)
    }
    fn asynchronous(self) -> bool {
        matches!(self, Self::Async | Self::AsyncArrow | Self::AsyncGenerator)
    }
    fn generator(self) -> bool {
        matches!(self, Self::Generator | Self::AsyncGenerator)
    }
}

#[derive(Clone, Default)]
struct Context {
    strict: bool,
    module: bool,
    import_meta: bool,
    top_level: bool,
    function: bool,
    kind: FunctionKind,
    await_allowed: bool,
    await_reserved: bool,
    yield_allowed: bool,
    parameters: bool,
    super_call: bool,
    super_property: bool,
    private_names: Vec<Rc<HashSet<String>>>,
    forbid_arguments: bool,
    lexical_functions: bool,
    new_target: bool,
    loops: usize,
    switches: usize,
    case_clause: bool,
    labels: Vec<(String, bool)>,
}

type Check = Result<(), String>;

#[derive(Default)]
pub(crate) struct EvalContext {
    pub forbid_arguments: bool,
    pub new_target: bool,
    pub strict: bool,
    pub super_call: bool,
    pub super_property: bool,
    pub private_names: HashSet<String>,
}

fn directive_literal(statement: &Statement) -> bool {
    matches!(statement, Statement::Expr(expr) if expr.is_string_literal())
}

pub(crate) fn use_strict(body: &[Statement]) -> bool {
    body.iter()
        .take_while(|s| directive_literal(s))
        .any(|s| matches!(s, Statement::Expr(Expr::String(text)) if text == "use strict"))
}

pub(super) fn validate(
    body: &[Statement],
    new_target: bool,
    strict: bool,
    goal: ParseGoal,
    eval: Option<&EvalContext>,
) -> Check {
    fn module_declaration(statement: &Statement) -> bool {
        match statement {
            Statement::Import { .. }
            | Statement::ExportNamed { .. }
            | Statement::ExportAll { .. }
            | Statement::ExportDefault(_) => true,
            Statement::Declarations(body) => body.iter().any(module_declaration),
            _ => false,
        }
    }
    let inferred_module = body.iter().any(module_declaration);
    let module = goal == ParseGoal::Module || goal == ParseGoal::Auto && inferred_module;
    if module {
        module_exports(body)?;
    }
    statements(
        body,
        &Context {
            strict: strict || module || use_strict(body),
            module,
            import_meta: goal != ParseGoal::Script,
            top_level: true,
            new_target,
            await_allowed: goal != ParseGoal::Script,
            lexical_functions: module,
            forbid_arguments: eval.is_some_and(|context| context.forbid_arguments),
            super_call: eval.is_some_and(|context| context.super_call),
            super_property: eval.is_some_and(|context| context.super_property),
            private_names: eval
                .map(|context| vec![Rc::new(context.private_names.clone())])
                .unwrap_or_default(),
            ..Context::default()
        },
    )
}

fn module_exports(body: &[Statement]) -> Check {
    fn direct<'a>(body: &'a [Statement], out: &mut Vec<&'a Statement>) {
        for stmt in body {
            if let Statement::Declarations(body) = stmt {
                direct(body, out);
            } else {
                out.push(stmt);
            }
        }
    }
    let mut declarations = Vec::new();
    direct(body, &mut declarations);
    let mut bound = lexical_names(body, true, false)?;
    let mut vars = Vec::new();
    collect_var_declaration_names(body, &mut vars);
    bound.extend(vars);
    let mut exports = HashSet::new();
    for stmt in declarations {
        match stmt {
            Statement::ExportDefault(_) => {
                if !exports.insert("default".to_owned()) {
                    return Err("duplicate default export".into());
                }
            }
            Statement::ExportNamed {
                specifiers, source, ..
            } => {
                for (local, exported) in specifiers {
                    if !exports.insert(exported.clone()) {
                        return Err(format!("duplicate export: {exported}"));
                    }
                    if source.is_none() && !bound.contains(local) {
                        return Err(format!("export of undeclared binding: {local}"));
                    }
                }
            }
            Statement::ExportAll {
                alias: Some(alias), ..
            } if !exports.insert(alias.clone()) => {
                return Err(format!("duplicate export: {alias}"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn reserved_identifier(name: &str) -> bool {
    matches!(
        name,
        "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "new"
            | "null"
            | "return"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
    )
}

fn binding(name: &str, ctx: &Context) -> Check {
    if reserved_identifier(name) {
        return Err(format!("reserved binding: {name}"));
    }
    if name == "await" && (ctx.module || ctx.kind.asynchronous() || ctx.await_reserved)
        || name == "yield" && ctx.kind.generator()
    {
        return Err(format!("reserved contextual binding: {name}"));
    }
    if ctx.strict
        && matches!(
            name,
            "eval"
                | "arguments"
                | "implements"
                | "interface"
                | "let"
                | "package"
                | "private"
                | "protected"
                | "public"
                | "static"
                | "yield"
        )
    {
        return Err(format!("invalid strict-mode binding: {name}"));
    }
    Ok(())
}

fn parameters(params: &[String], ctx: &Context, unique: bool) -> Check {
    let mut names = HashSet::new();
    for param in params {
        let name = param.trim_start_matches("...");
        binding(name, ctx)?;
        if !names.insert(name) && (ctx.strict || unique) {
            return Err(format!("duplicate parameter: {name}"));
        }
    }
    Ok(())
}

fn function(
    params: &[String],
    body: &[Statement],
    outer: &Context,
    kind: FunctionKind,
    unique: bool,
    method: bool,
) -> Check {
    let ctx = Context {
        strict: outer.strict || use_strict(body),
        function: true,
        kind,
        module: outer.module,
        import_meta: outer.import_meta,
        new_target: !kind.arrow() || outer.new_target,
        await_allowed: kind.asynchronous(),
        yield_allowed: kind.generator(),
        super_call: (kind.arrow() || method) && outer.super_call,
        super_property: method || kind.arrow() && outer.super_property,
        private_names: outer.private_names.clone(),
        forbid_arguments: kind.arrow() && outer.forbid_arguments,
        ..Context::default()
    };
    let non_simple = params.iter().any(|p| p.starts_with("..."))
        || matches!(body.first(), Some(Statement::ParameterInitialization { initializers, .. }) if !initializers.is_empty());
    let unique_bindings = unique || kind.arrow() || ctx.strict || non_simple;
    parameters(params, &ctx, unique_bindings)?;
    // Parameters cannot collide with direct lexical declarations in the body.
    let lexical = lexical_names(body, ctx.lexical_functions, !ctx.strict && !ctx.module)?;
    let mut parameter_names: Vec<_> = params
        .iter()
        .map(|p| p.trim_start_matches("...").to_owned())
        .collect();
    if let Some(Statement::ParameterInitialization { initializers, .. }) = body.first() {
        for initializer in initializers {
            if let Statement::VarDecl {
                destructuring: Some(pattern),
                init,
                ..
            } = initializer
            {
                if let Some(Expr::Identifier(slot)) = init.as_deref() {
                    parameter_names.retain(|name| name != slot);
                }
                parameter_names.extend(pattern_names(pattern));
            }
        }
    }
    if unique_bindings {
        let mut seen = HashSet::new();
        if parameter_names.iter().any(|name| !seen.insert(name)) {
            return Err("duplicate parameter binding".into());
        }
    }
    if parameter_names.iter().any(|p| lexical.contains(p)) {
        return Err("parameter conflicts with lexical declaration".into());
    }
    for stmt in body {
        if let Statement::ParameterInitialization { initializers, .. } = stmt {
            for initializer in initializers {
                statement(
                    initializer,
                    &Context {
                        parameters: true,
                        await_reserved: kind.arrow() && outer.await_reserved,
                        ..ctx.clone()
                    },
                )?;
            }
        }
    }
    statements(body, &ctx)
}

fn lexical_names(
    body: &[Statement],
    functions: bool,
    sloppy: bool,
) -> Result<HashSet<String>, String> {
    let mut names = HashSet::new();
    let mut ordinary_functions = HashSet::new();
    for stmt in body {
        match stmt {
            Statement::Declarations(decls)
            | Statement::ResourceDeclaration {
                declarations: decls,
                ..
            } => {
                for name in lexical_names(decls, functions, sloppy)? {
                    if !names.insert(name.clone()) {
                        return Err(format!("duplicate lexical binding: {name}"));
                    }
                }
            }
            Statement::VarDecl {
                kind: VarKind::Let | VarKind::Const,
                name,
                destructuring,
                ..
            } => {
                let bound = destructuring
                    .as_deref()
                    .map(pattern_names)
                    .unwrap_or_else(|| vec![name.clone()]);
                for name in bound {
                    if !names.insert(name.clone()) {
                        return Err(format!("duplicate lexical binding: {name}"));
                    }
                }
            }
            Statement::FnDecl {
                name,
                is_async,
                is_generator,
                ..
            } if functions => {
                let ordinary = !is_async && !is_generator;
                if !names.insert(name.clone())
                    && !(sloppy && ordinary && ordinary_functions.contains(name))
                {
                    return Err(format!("duplicate lexical binding: {name}"));
                }
                if ordinary {
                    ordinary_functions.insert(name.clone());
                }
            }
            Statement::Import {
                default,
                named,
                namespace,
                ..
            } => {
                for name in default
                    .iter()
                    .chain(namespace.iter())
                    .chain(named.iter().map(|(_, name)| name))
                {
                    if !names.insert(name.clone()) {
                        return Err(format!("duplicate import binding: {name}"));
                    }
                }
            }
            Statement::ClassDecl { name, .. } if !names.insert(name.clone()) => {
                return Err(format!("duplicate lexical binding: {name}"));
            }
            _ => {}
        }
    }
    Ok(names)
}

fn statements(body: &[Statement], ctx: &Context) -> Check {
    let lexical = lexical_names(body, ctx.lexical_functions, !ctx.strict && !ctx.module)?;
    let mut vars = Vec::new();
    collect_var_declaration_names(body, &mut vars);
    for statement in body {
        if !ctx.lexical_functions
            && let Statement::FnDecl { name, .. } = statement
        {
            vars.push(name.clone());
        }
    }
    if let Some(name) = vars.iter().find(|name| lexical.contains(*name)) {
        return Err(format!(
            "var/function declaration conflicts with lexical binding: {name}"
        ));
    }
    for stmt in body {
        statement(stmt, ctx)?;
    }
    Ok(())
}

fn nested_statements(body: &[Statement], ctx: &Context) -> Check {
    statements(
        body,
        &Context {
            top_level: false,
            lexical_functions: true,
            case_clause: false,
            ..ctx.clone()
        },
    )
}

fn optional(expr: Option<&Expr>, ctx: &Context) -> Check {
    if let Some(expr) = expr {
        expression(expr, ctx)?;
    }
    Ok(())
}

fn statement(stmt: &Statement, ctx: &Context) -> Check {
    if matches!(
        stmt,
        Statement::Import { .. }
            | Statement::ExportNamed { .. }
            | Statement::ExportAll { .. }
            | Statement::ExportDefault(_)
    ) && (!ctx.module || !ctx.top_level)
    {
        return Err("import/export declaration outside module top level".into());
    }
    match stmt {
        Statement::Expr(expr) => expression(expr, ctx),
        Statement::Throw(expr) | Statement::ExportDefault(expr) => expression(expr, ctx),
        Statement::VarDecl {
            name,
            init,
            destructuring,
            kind,
        } => {
            if *kind == VarKind::Const && init.is_none() {
                return Err("const declaration requires an initializer".into());
            }
            if matches!(kind, VarKind::Let | VarKind::Const)
                && destructuring
                    .as_deref()
                    .map(pattern_names)
                    .unwrap_or_else(|| vec![name.clone()])
                    .iter()
                    .any(|name| name == "let")
            {
                return Err("let is not a lexical binding name".into());
            }
            if let Some(pattern) = destructuring {
                pattern_check(pattern, ctx)?;
            } else {
                binding(name, ctx)?;
            }
            optional(init.as_deref(), ctx)
        }
        Statement::FnDecl {
            name,
            params,
            body,
            is_async,
            is_generator,
            annex_b_statement,
        } => {
            if *annex_b_statement && ctx.strict {
                return Err("legacy function declaration in strict code".into());
            }
            let mut own = ctx.clone();
            own.strict |= use_strict(body);
            binding(name, &own)?;
            function(
                params,
                body,
                ctx,
                FunctionKind::of(false, *is_async, *is_generator),
                *is_async || *is_generator,
                false,
            )
        }
        Statement::ResourceForOf {
            name,
            iter,
            body,
            is_await,
            await_disposal,
        } => {
            if (*is_await || *await_disposal) && !ctx.await_allowed {
                return Err("async resource loop outside async context".into());
            }
            binding(name, ctx)?;
            let mut vars = Vec::new();
            collect_var_declaration_names(body, &mut vars);
            if vars.iter().any(|var| var == name) {
                return Err("resource loop binding conflicts with var declaration".into());
            }
            expression(iter, ctx)?;
            statements(
                body,
                &Context {
                    loops: ctx.loops + 1,
                    ..ctx.clone()
                },
            )
        }
        Statement::ResourceDeclaration {
            declarations,
            is_await,
        } => {
            if ctx.case_clause || ctx.top_level && !ctx.module {
                return Err(
                    "resource declaration requires a block, function or module scope".into(),
                );
            }
            if *is_await && !ctx.await_allowed {
                return Err("await using outside async context".into());
            }
            for declaration in declarations {
                statement(declaration, ctx)?;
            }
            Ok(())
        }
        Statement::With { object, body } => {
            if ctx.strict {
                return Err("with statement in strict code".into());
            }
            expression(object, ctx)?;
            statements(body, ctx)
        }
        Statement::ClassDecl {
            name,
            superclass,
            body,
        } => {
            binding(
                name,
                &Context {
                    strict: true,
                    ..ctx.clone()
                },
            )?;
            heritage(
                superclass.as_deref(),
                &Context {
                    strict: true,
                    ..ctx.clone()
                },
            )?;
            class(body, ctx, superclass.is_some())
        }
        Statement::Return(expr) => {
            if !ctx.function {
                return Err("return outside a function".into());
            }
            optional(expr.as_deref(), ctx)
        }
        Statement::If { test, then, else_ } => {
            expression(test, ctx)?;
            nested_statements(then, ctx)?;
            if let Some(body) = else_ {
                nested_statements(body, ctx)?;
            }
            Ok(())
        }
        Statement::While { test, body } | Statement::DoWhile { test, body } => {
            expression(test, ctx)?;
            nested_statements(
                body,
                &Context {
                    loops: ctx.loops + 1,
                    ..ctx.clone()
                },
            )
        }
        Statement::For {
            init,
            test,
            update,
            body,
        } => {
            if let Some(init) = init {
                match init.as_ref() {
                    ForInit::Expr(expr) => expression(expr, ctx)?,
                    ForInit::Var { kind, decls } => {
                        if *kind != VarKind::Var {
                            lexical_loop_head(
                                &decls
                                    .iter()
                                    .map(|(name, _)| name.clone())
                                    .collect::<Vec<_>>(),
                                body,
                            )?;
                        }
                        for (name, expr) in decls {
                            if *kind == VarKind::Const && expr.is_none() {
                                return Err("const declaration requires an initializer".into());
                            }
                            binding(name, ctx)?;
                            optional(expr.as_ref(), ctx)?;
                        }
                    }
                    ForInit::Pattern {
                        pattern,
                        init,
                        trailing,
                        kind,
                    } => {
                        if *kind != VarKind::Var {
                            let mut names = pattern_names(pattern);
                            names.extend(trailing.iter().map(|(name, _)| name.clone()));
                            lexical_loop_head(&names, body)?;
                        }
                        pattern_check(pattern, ctx)?;
                        expression(init, ctx)?;
                        for (name, expr) in trailing {
                            if *kind == VarKind::Const && expr.is_none() {
                                return Err("const declaration requires an initializer".into());
                            }
                            binding(name, ctx)?;
                            optional(expr.as_ref(), ctx)?;
                        }
                    }
                }
            }
            optional(test.as_deref(), ctx)?;
            optional(update.as_deref(), ctx)?;
            nested_statements(
                body,
                &Context {
                    loops: ctx.loops + 1,
                    ..ctx.clone()
                },
            )
        }
        Statement::ForIn { binding, obj, body } => {
            iteration_binding(binding, body, ctx, true)?;
            expression(obj, ctx)?;
            nested_statements(
                body,
                &Context {
                    loops: ctx.loops + 1,
                    ..ctx.clone()
                },
            )
        }
        Statement::ForOf {
            binding,
            iter,
            body,
            is_await,
        } => {
            if *is_await && (!ctx.await_allowed || ctx.parameters) {
                return Err("for await outside an async context".into());
            }
            iteration_binding(binding, body, ctx, false)?;
            expression(iter, ctx)?;
            nested_statements(
                body,
                &Context {
                    loops: ctx.loops + 1,
                    ..ctx.clone()
                },
            )
        }
        Statement::Block(body) => nested_statements(body, ctx),
        Statement::ClassInitialization { fields, .. } => nested_statements(fields, ctx),
        Statement::ParameterInitialization {
            initializers: body, ..
        }
        | Statement::Declarations(body) => {
            for stmt in body {
                statement(stmt, ctx)?;
            }
            Ok(())
        }
        Statement::Labeled { label, body } => {
            expression(&Expr::Identifier(label.clone()), ctx)?;
            if ctx.labels.iter().any(|(name, _)| name == label) {
                return Err(format!("duplicate label: {label}"));
            }
            let mut next = ctx.clone();
            next.top_level = false;
            let mut target = body.as_ref();
            while let Statement::Labeled { body, .. } = target {
                target = body;
            }
            let iteration = matches!(
                target,
                Statement::While { .. }
                    | Statement::DoWhile { .. }
                    | Statement::For { .. }
                    | Statement::ForOf { .. }
                    | Statement::ForIn { .. }
            );
            next.labels.push((label.clone(), iteration));
            statement(body, &next)
        }
        Statement::Break if ctx.loops == 0 && ctx.switches == 0 => {
            Err("break outside a loop or switch".into())
        }
        Statement::Continue if ctx.loops == 0 => Err("continue outside a loop".into()),
        Statement::LabeledBreak(label) | Statement::LabeledContinue(label) => {
            let iteration = matches!(stmt, Statement::LabeledContinue(_));
            if !ctx
                .labels
                .iter()
                .any(|(name, is_loop)| name == label && (!iteration || *is_loop))
            {
                return Err(format!("invalid control-flow label: {label}"));
            }
            Ok(())
        }
        Statement::Try {
            body,
            catch,
            finally,
        } => {
            nested_statements(body, ctx)?;
            if let Some((pattern, body)) = catch {
                if let Some(pattern) = pattern {
                    pattern_check(pattern, ctx)?;
                    let names = pattern_names(pattern);
                    let mut seen = HashSet::new();
                    if names.iter().any(|name| !seen.insert(name)) {
                        return Err("duplicate catch parameter binding".into());
                    }
                    let lexical = lexical_names(body, true, false)?;
                    if names.iter().any(|name| lexical.contains(name)) {
                        return Err("catch parameter conflicts with a lexical declaration".into());
                    }
                    if !matches!(pattern, Pattern::Ident(_)) {
                        let mut vars = Vec::new();
                        collect_var_declaration_names(body, &mut vars);
                        if names.iter().any(|name| vars.contains(name)) {
                            return Err("catch pattern conflicts with a var declaration".into());
                        }
                    }
                }
                nested_statements(body, ctx)?;
            }
            if let Some(body) = finally {
                nested_statements(body, ctx)?;
            }
            Ok(())
        }
        Statement::Switch { disc, cases } => {
            expression(disc, ctx)?;
            let next = Context {
                switches: ctx.switches + 1,
                case_clause: true,
                ..ctx.clone()
            };
            if cases.iter().filter(|case| case.test.is_none()).count() > 1 {
                return Err("duplicate default clause".into());
            }
            let body: Vec<_> = cases
                .iter()
                .flat_map(|case| case.body.iter().cloned())
                .collect();
            statements(
                &body,
                &Context {
                    top_level: false,
                    lexical_functions: true,
                    ..next.clone()
                },
            )?;
            for case in cases {
                optional(case.test.as_ref(), ctx)?;
                nested_statements(&case.body, &next)?;
            }
            Ok(())
        }
        Statement::Import {
            default,
            named,
            namespace,
            ..
        } => {
            for name in default
                .iter()
                .chain(namespace.iter())
                .chain(named.iter().map(|(_, name)| name))
            {
                binding(name, ctx)?;
            }
            Ok(())
        }
        Statement::Break
        | Statement::Continue
        | Statement::ExportNamed { .. }
        | Statement::ExportAll { .. }
        | Statement::Empty => Ok(()),
    }
}

fn pattern_check(pattern: &Pattern, ctx: &Context) -> Check {
    match pattern {
        Pattern::Elision => Ok(()),
        Pattern::Ident(name) => binding(name, ctx),
        Pattern::Rest(inner) => {
            if matches!(inner.as_ref(), Pattern::Default(..)) {
                return Err("rest binding cannot have an initializer".into());
            }
            pattern_check(inner, ctx)
        }
        Pattern::Default(inner, value) => {
            pattern_check(inner, ctx)?;
            expression(value, ctx)
        }
        Pattern::Array(items) => {
            for item in items {
                pattern_check(item, ctx)?;
            }
            Ok(())
        }
        Pattern::Object(props) => {
            for (index, (key, value)) in props.iter().enumerate() {
                if let Some(Pattern::Rest(inner)) = value
                    && (index + 1 != props.len() || !matches!(inner.as_ref(), Pattern::Ident(_)))
                {
                    return Err("object binding rest must be a final identifier".into());
                }
                if let PatternKey::Computed(expr) = key {
                    expression(expr, ctx)?;
                }
                if let Some(value) = value {
                    pattern_check(value, ctx)?;
                } else if let PatternKey::Name(name) = key {
                    binding(name, ctx)?;
                }
            }
            Ok(())
        }
        Pattern::Member {
            object, property, ..
        } => {
            expression(object, ctx)?;
            expression(property, ctx)
        }
    }
}

fn lexical_loop_head(names: &[String], body: &[Statement]) -> Check {
    let mut seen = HashSet::new();
    if names.iter().any(|name| name == "let" || !seen.insert(name)) {
        return Err("invalid lexical loop binding".into());
    }
    let mut vars = Vec::new();
    collect_var_declaration_names(body, &mut vars);
    if names.iter().any(|name| vars.contains(name)) {
        return Err("loop binding conflicts with var declaration".into());
    }
    Ok(())
}

fn iteration_binding(
    binding: &ForBinding,
    body: &[Statement],
    ctx: &Context,
    for_in: bool,
) -> Check {
    match binding {
        ForBinding::Assignment(target) => match target.as_ref() {
            Expr::Array { .. } | Expr::Object { .. } => assignment_target(target, ctx),
            _ => legacy_assignment_target(target, ctx),
        },
        ForBinding::Declaration {
            kind,
            pattern,
            initializer,
        } => {
            pattern_check(pattern, ctx)?;
            if initializer.is_some()
                && (!for_in
                    || ctx.strict
                    || *kind != VarKind::Var
                    || !matches!(pattern, Pattern::Ident(_)))
            {
                return Err("invalid iteration declaration initializer".into());
            }
            optional(initializer.as_deref(), ctx)?;
            if *kind != VarKind::Var {
                let names = pattern_names(pattern);
                lexical_loop_head(&names, body)?;
            }
            Ok(())
        }
    }
}

fn contains_optional_chain(expr: &Expr) -> bool {
    match expr {
        Expr::OptionalChain { .. } => true,
        Expr::Member { object, .. } => contains_optional_chain(object),
        Expr::Call { callee, .. } => contains_optional_chain(callee),
        _ => false,
    }
}

fn simple_assignment_target(target: &Expr, ctx: &Context) -> Check {
    match target {
        Expr::Parenthesized(inner) => simple_assignment_target(inner, ctx),
        Expr::Identifier(name) if !name.starts_with('#') => binding(name, ctx),
        Expr::Member { .. } if !contains_optional_chain(target) => expression(target, ctx),
        _ => Err("invalid assignment target".into()),
    }
}

fn legacy_assignment_target(target: &Expr, ctx: &Context) -> Check {
    if !ctx.strict
        && matches!(target.unparenthesized(), Expr::Call { .. })
        && !contains_optional_chain(target)
    {
        return expression(target, ctx);
    }
    simple_assignment_target(target, ctx)
}

fn assignment_target(target: &Expr, ctx: &Context) -> Check {
    match target {
        Expr::LegacyLiteral(inner) => {
            if ctx.strict {
                return Err("legacy literal in strict code".into());
            }
            assignment_target(inner, ctx)
        }
        Expr::Array {
            items,
            trailing_comma,
        } => {
            for (index, item) in items.iter().enumerate() {
                match item {
                    Expr::Undefined => {}
                    Expr::Spread(inner) if index + 1 == items.len() && !trailing_comma => {
                        if matches!(inner.as_ref(), Expr::Assignment { .. }) {
                            return Err("rest element cannot have an initializer".into());
                        }
                        assignment_target(inner, ctx)?
                    }
                    Expr::Spread(_) => return Err("rest element must be last".into()),
                    _ => assignment_target(item, ctx)?,
                }
            }
            Ok(())
        }
        Expr::Object {
            props,
            trailing_comma,
        } => {
            for (index, prop) in props.iter().enumerate() {
                match prop {
                    ObjectProp::CoverInitializedName { name, initializer } => {
                        binding(name, ctx)?;
                        expression(initializer, ctx)?;
                    }
                    ObjectProp::Shorthand(name) => binding(name, ctx)?,
                    ObjectProp::KeyValue(_, target) => assignment_target(target, ctx)?,
                    ObjectProp::Computed(key, target) => {
                        expression(key, ctx)?;
                        assignment_target(target, ctx)?;
                    }
                    ObjectProp::Spread(target) if index + 1 == props.len() && !trailing_comma => {
                        simple_assignment_target(target, ctx)?
                    }
                    _ => return Err("invalid object assignment pattern".into()),
                }
            }
            Ok(())
        }
        Expr::Assignment {
            target,
            value,
            op: AssignOp::Assign,
        } => {
            assignment_target(target, ctx)?;
            expression(value, ctx)
        }
        _ => simple_assignment_target(target, ctx),
    }
}

fn argument(expr: &Expr, ctx: &Context) -> Check {
    match expr {
        Expr::Spread(inner) => expression(inner, ctx),
        _ => expression(expr, ctx),
    }
}

fn expression(expr: &Expr, ctx: &Context) -> Check {
    match expr {
        Expr::Parenthesized(inner) => expression(inner, ctx),
        Expr::Regex(pattern, flags) => crate::regex::validate_syntax(pattern, flags).map(|_| ()),
        Expr::LegacyLiteral(inner) => {
            if ctx.strict {
                return Err("legacy literal in strict code".into());
            }
            expression(inner, ctx)
        }
        Expr::Identifier(name) if name == "arguments" && ctx.forbid_arguments => {
            Err("arguments in class initialization".into())
        }
        Expr::Identifier(name)
            if name == "await" && (ctx.module || ctx.kind.asynchronous() || ctx.await_reserved) =>
        {
            Err("await used as an identifier".into())
        }
        Expr::Identifier(name) if name == "yield" && (ctx.strict || ctx.kind.generator()) => {
            Err("yield used as an identifier".into())
        }
        Expr::Identifier(name)
            if ctx.strict
                && matches!(
                    name.as_str(),
                    "implements"
                        | "interface"
                        | "let"
                        | "package"
                        | "private"
                        | "protected"
                        | "public"
                        | "static"
                ) =>
        {
            Err(format!("reserved strict identifier: {name}"))
        }
        Expr::Identifier(name) if reserved_identifier(name) => {
            Err(format!("reserved identifier: {name}"))
        }
        Expr::Identifier(name) if name.starts_with('#') => {
            Err("private name must be the left operand of in".into())
        }
        Expr::Super => Err("bare super expression".into()),
        Expr::ImportMeta if !ctx.import_meta => Err("import.meta outside a module".into()),
        Expr::NewTarget if !ctx.new_target => Err("new.target outside a function".into()),
        Expr::Array { items, .. } => {
            for item in items {
                argument(item, ctx)?;
            }
            Ok(())
        }
        Expr::Template { exprs, .. } => {
            for expr in exprs {
                expression(expr, ctx)?;
            }
            Ok(())
        }
        Expr::Object { props, .. } => {
            if props
                .iter()
                .filter(|prop| matches!(prop, ObjectProp::KeyValue(name, _) if name == "__proto__"))
                .count()
                > 1
            {
                return Err("duplicate prototype setter".into());
            }
            for prop in props {
                match prop {
                    ObjectProp::CoverInitializedName { .. } => {
                        return Err("cover initialized name outside an assignment pattern".into());
                    }
                    ObjectProp::Shorthand(name) => {
                        expression(&Expr::Identifier(name.clone()), ctx)?;
                    }
                    ObjectProp::KeyValue(_, value) | ObjectProp::Spread(value) => {
                        expression(value, ctx)?
                    }
                    ObjectProp::Computed(key, value) => {
                        expression(key, ctx)?;
                        expression(value, ctx)?;
                    }
                    ObjectProp::ComputedMethod {
                        key,
                        params,
                        body,
                        is_async,
                        is_generator,
                        ..
                    } => {
                        expression(key, ctx)?;
                        function(
                            params,
                            body,
                            ctx,
                            FunctionKind::of(false, *is_async, *is_generator),
                            true,
                            true,
                        )?;
                    }
                    ObjectProp::Method {
                        params,
                        body,
                        is_async,
                        is_generator,
                        ..
                    } => function(
                        params,
                        body,
                        ctx,
                        FunctionKind::of(false, *is_async, *is_generator),
                        true,
                        true,
                    )?,
                    ObjectProp::Getter { body, .. } => {
                        function(&[], body, ctx, FunctionKind::Ordinary, true, true)?
                    }
                    ObjectProp::Setter { param, body, .. } => function(
                        std::slice::from_ref(param),
                        body,
                        ctx,
                        FunctionKind::Ordinary,
                        true,
                        true,
                    )?,
                }
            }
            Ok(())
        }
        Expr::Binary {
            op: BinOp::In,
            left,
            right,
        } if matches!(left.as_ref(), Expr::Identifier(name) if name.starts_with('#')) => {
            if let Expr::Identifier(name) = left.as_ref() {
                private_reference(name, ctx)?;
            }
            expression(right, ctx)
        }
        Expr::Binary { op, left, right } => {
            if *op == BinOp::Pow
                && matches!(left.as_ref(), Expr::Unary { op, .. } if !matches!(op, UnOp::Inc | UnOp::Dec))
            {
                return Err("unparenthesized unary expression before exponentiation".into());
            }
            if *op == BinOp::Nullish
                && [left.as_ref(), right.as_ref()].iter().any(|expr| {
                    matches!(
                        expr,
                        Expr::Binary {
                            op: BinOp::And | BinOp::Or,
                            ..
                        }
                    )
                })
            {
                return Err("nullish coalescing mixed with logical operators".into());
            }
            expression(left, ctx)?;
            expression(right, ctx)
        }
        Expr::Member {
            object,
            property,
            computed,
        }
        | Expr::OptionalChain {
            object,
            property,
            computed,
        } => {
            if matches!(object.as_ref(), Expr::Super) {
                if !ctx.super_property
                    || matches!(expr, Expr::OptionalChain { .. })
                    || private_member(expr).is_some()
                {
                    return Err("invalid super property access".into());
                }
            } else {
                expression(object, ctx)?;
            }
            if let Some(name) = private_member(expr) {
                private_reference(&name, ctx)?;
            }
            if *computed {
                expression(property, ctx)?;
            }
            Ok(())
        }
        Expr::Unary { op, operand, .. } => {
            if *op == UnOp::Delete && private_member(operand.unparenthesized()).is_some() {
                return Err("delete of a private element".into());
            }
            if ctx.strict
                && *op == UnOp::Delete
                && matches!(operand.unparenthesized(), Expr::Identifier(_))
            {
                return Err("delete of an unqualified identifier in strict mode".into());
            }
            if matches!(op, UnOp::Inc | UnOp::Dec) {
                legacy_assignment_target(operand, ctx)
            } else {
                expression(operand, ctx)
            }
        }
        Expr::Assignment { target, value, op } => {
            if *op == AssignOp::Assign
                && matches!(target.as_ref(), Expr::Array { .. } | Expr::Object { .. })
            {
                assignment_target(target, ctx)?;
            } else {
                legacy_assignment_target(target, ctx)?;
            }
            expression(value, ctx)
        }
        Expr::LogicalAssignment { target, value, .. } => {
            simple_assignment_target(target, ctx)?;
            expression(value, ctx)
        }
        Expr::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expression(test, ctx)?;
            expression(consequent, ctx)?;
            expression(alternate, ctx)
        }
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            if matches!(callee.as_ref(), Expr::Super) {
                if !ctx.super_call || matches!(expr, Expr::New { .. }) {
                    return Err("super call outside a derived constructor".into());
                }
            } else {
                expression(callee, ctx)?;
            }
            for arg in args {
                argument(arg, ctx)?;
            }
            Ok(())
        }
        Expr::TaggedTemplate { tag, exprs, .. } => {
            if contains_optional_chain(tag) {
                return Err("optional chain cannot be a template tag".into());
            }
            expression(tag, ctx)?;
            for expr in exprs {
                expression(expr, ctx)?;
            }
            Ok(())
        }
        Expr::ArrowFn {
            params,
            body,
            is_async,
        } => match body.as_ref() {
            ExprOrBlock::Block(body) => function(
                params,
                body,
                ctx,
                FunctionKind::of(true, *is_async, false),
                true,
                false,
            ),
            ExprOrBlock::Expr(expr) => {
                parameters(
                    params,
                    &Context {
                        kind: FunctionKind::of(true, *is_async, false),
                        ..ctx.clone()
                    },
                    true,
                )?;
                expression(
                    expr,
                    &Context {
                        function: true,
                        kind: FunctionKind::of(true, *is_async, false),
                        await_allowed: *is_async,
                        await_reserved: false,
                        yield_allowed: false,
                        parameters: false,
                        loops: 0,
                        switches: 0,
                        labels: Vec::new(),
                        ..ctx.clone()
                    },
                )
            }
        },
        Expr::FnExpr {
            name,
            params,
            body,
            is_async,
            is_generator,
        } => {
            let mut own = ctx.clone();
            own.strict |= use_strict(body);
            own.await_reserved = false;
            own.kind = FunctionKind::of(false, *is_async, *is_generator);
            if let Some(name) = name {
                binding(name, &own)?;
            }
            function(
                params,
                body,
                ctx,
                FunctionKind::of(false, *is_async, *is_generator),
                *is_async || *is_generator,
                false,
            )
        }
        Expr::ClassExpr {
            name,
            superclass,
            body,
        } => {
            if let Some(name) = name {
                binding(
                    name,
                    &Context {
                        strict: true,
                        ..ctx.clone()
                    },
                )?;
            }
            heritage(
                superclass.as_deref(),
                &Context {
                    strict: true,
                    ..ctx.clone()
                },
            )?;
            class(body, ctx, superclass.is_some())
        }
        Expr::Await(value) => {
            if !ctx.await_allowed || ctx.parameters {
                return Err("await outside an async context or in parameters".into());
            }
            expression(value, ctx)
        }
        Expr::YieldFrom(value) => {
            if !ctx.yield_allowed || ctx.parameters {
                return Err("yield outside a generator or in parameters".into());
            }
            expression(value, ctx)
        }
        Expr::Yield(value) => {
            if !ctx.yield_allowed || ctx.parameters {
                return Err("yield outside a generator or in parameters".into());
            }
            optional(value.as_deref(), ctx)
        }
        Expr::Spread(_) => Err("spread outside an array or argument list".into()),
        Expr::DynamicImport {
            specifier, options, ..
        } => {
            expression(specifier, ctx)?;
            optional(options.as_deref(), ctx)
        }
        Expr::Number(_)
        | Expr::BigIntLiteral(_)
        | Expr::String(_)
        | Expr::EscapedString(_)
        | Expr::Bool(_)
        | Expr::Null
        | Expr::Undefined
        | Expr::Identifier(_)
        | Expr::This
        | Expr::ImportMeta
        | Expr::NewTarget => Ok(()),
    }
}

fn private_reference(name: &str, ctx: &Context) -> Check {
    if !ctx
        .private_names
        .iter()
        .rev()
        .any(|scope| scope.contains(name))
    {
        return Err(format!("undeclared private name: {name}"));
    }
    Ok(())
}

fn private_member(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Member {
            property,
            computed: false,
            ..
        }
        | Expr::OptionalChain {
            property,
            computed: false,
            ..
        } => match property.as_ref() {
            Expr::String(name) if name.starts_with("#") => Some(name.to_key()),
            _ => None,
        },
        _ => None,
    }
}

fn heritage(superclass: Option<&Expr>, ctx: &Context) -> Check {
    if let Some(expr) = superclass {
        if matches!(
            expr,
            Expr::ArrowFn { .. }
                | Expr::Assignment { .. }
                | Expr::LogicalAssignment { .. }
                | Expr::Binary { .. }
                | Expr::Conditional { .. }
                | Expr::Unary { .. }
                | Expr::Await(_)
                | Expr::Yield(_)
                | Expr::YieldFrom(_)
                | Expr::Spread(_)
        ) {
            return Err("class heritage requires a left-hand-side expression".into());
        }
        expression(expr, ctx)?;
    }
    Ok(())
}

fn class(body: &[ClassMember], outer: &Context, derived: bool) -> Check {
    let mut ctx = Context {
        strict: true,
        ..outer.clone()
    };
    let mut private = HashMap::new();
    let mut constructors = 0;
    for member in body {
        let (name, is_static, accessor) = match member {
            ClassMember::Method {
                name, is_static, ..
            }
            | ClassMember::Field {
                name, is_static, ..
            } => (name, *is_static, 0),
            ClassMember::Getter {
                name, is_static, ..
            } => (name, *is_static, 1),
            ClassMember::Setter {
                name, is_static, ..
            } => (name, *is_static, 2),
            ClassMember::StaticBlock { .. } => continue,
        };
        if let MemberName::Private(name) = name {
            if name == "#constructor" {
                return Err("private constructor name is forbidden".into());
            }
            if let Some((previous_static, previous_accessor)) = private.get(name) {
                if *previous_static != is_static
                    || accessor == 0
                    || *previous_accessor == 0
                    || *previous_accessor & accessor != 0
                {
                    return Err(format!("duplicate private name: {name}"));
                }
                private.insert(name.clone(), (is_static, *previous_accessor | accessor));
            } else {
                private.insert(name.clone(), (is_static, accessor));
            }
        }
        if let MemberName::Static(name) = name {
            if name == "constructor" && matches!(member, ClassMember::Field { .. }) {
                return Err("constructor field name is forbidden".into());
            }
            if is_static && name == "prototype" {
                return Err("static prototype element is forbidden".into());
            }
            if !is_static && name == "constructor" {
                match member {
                    ClassMember::Method {
                        is_async: false,
                        is_generator: false,
                        ..
                    } => {
                        constructors += 1;
                        if constructors > 1 {
                            return Err("duplicate constructor".into());
                        }
                    }
                    _ => return Err("invalid constructor element".into()),
                }
            }
        }
    }
    ctx.private_names
        .push(Rc::new(private.into_keys().collect()));
    for member in body {
        let name = match member {
            ClassMember::Method { name, .. }
            | ClassMember::Field { name, .. }
            | ClassMember::Getter { name, .. }
            | ClassMember::Setter { name, .. } => Some(name),
            ClassMember::StaticBlock { .. } => None,
        };
        if let Some(MemberName::Computed(expr)) = name {
            expression(expr, &ctx)?;
        }
        match member {
            ClassMember::Method {
                name,
                is_static,
                params,
                body,
                is_async,
                is_generator,
            } => {
                let constructor =
                    !is_static && matches!(name, MemberName::Static(name) if name == "constructor");
                let method_ctx = Context {
                    super_call: constructor && derived,
                    ..ctx.clone()
                };
                function(
                    params,
                    body,
                    &method_ctx,
                    FunctionKind::of(false, *is_async, *is_generator),
                    true,
                    true,
                )?;
            }
            ClassMember::Getter { body, .. } => {
                function(&[], body, &ctx, FunctionKind::Ordinary, true, true)?
            }
            ClassMember::Setter { param, body, .. } => function(
                std::slice::from_ref(param),
                body,
                &ctx,
                FunctionKind::Ordinary,
                true,
                true,
            )?,
            ClassMember::Field { init, .. } => optional(
                init.as_ref(),
                &Context {
                    new_target: true,
                    super_property: true,
                    super_call: false,
                    await_allowed: false,
                    await_reserved: false,
                    yield_allowed: false,
                    forbid_arguments: true,
                    kind: FunctionKind::Ordinary,
                    parameters: false,
                    ..ctx.clone()
                },
            )?,
            ClassMember::StaticBlock { body } => statements(
                body,
                &Context {
                    new_target: true,
                    function: false,
                    super_property: true,
                    super_call: false,
                    await_allowed: false,
                    await_reserved: true,
                    yield_allowed: false,
                    forbid_arguments: true,
                    kind: FunctionKind::Ordinary,
                    parameters: false,
                    loops: 0,
                    switches: 0,
                    labels: Vec::new(),
                    top_level: false,
                    ..ctx.clone()
                },
            )?,
        }
    }
    Ok(())
}
