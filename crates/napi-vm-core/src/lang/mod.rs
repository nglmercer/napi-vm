//! Language services for the VM: completion, diagnostics, and document
//! symbols, built directly on the lexer/parser/AST.
//!
//! This module is the single, frontend-agnostic implementation of editor
//! intelligence. Every surface — the in-browser WASM playground, the LSP
//! server, and native GUIs (egui/eframe/slint) — calls these same functions,
//! so completion and analysis logic is written once and never recreated per
//! frontend.
//!
//! All entry points are pure functions of their inputs (source text + offset +
//! an [`AnalysisContext`] describing host-exposed functions and registered
//! modules), which keeps them trivially testable and safe to call from any
//! host.

pub mod actions;
pub mod catalog;
mod complete;
mod diagnostics;
mod document;
pub mod format_source;
mod metadata;
pub mod navigation;
mod scope;
pub mod semantic;
mod service;
mod symbols;

pub use complete::complete;
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub use complete::member_trigger;
pub use diagnostics::diagnose;
pub use document::{Document, HoverInfo, Type};
pub use metadata::clamp_type_name;
pub use metadata::{GlobalInfo, ParameterInfo, PropertyInfo, Shape};
#[cfg(not(target_arch = "wasm32"))]
pub use metadata::{
    MAX_DOCUMENTATION_BYTES, MAX_MANIFEST_BYTES, MAX_NAME_LENGTH, MAX_PARAMETERS, parse_globals,
};
pub use service::LanguageService;

use crate::lexer::Lexer;
use crate::parser::Parser;
use std::collections::HashMap;

/// Virtual module namespace used by playground completion.
///
/// This is a language-level specifier, not a filesystem path. Keeping it in
/// the language module prevents completion code from coupling itself to the
/// playground's current directory layout.
pub const MODULE_NAMESPACE_PREFIX: &str = "@playground/";

/// Extract a module name from a virtual playground module specifier.
pub fn playground_module_name(specifier: &str) -> Option<&str> {
    specifier
        .strip_prefix(MODULE_NAMESPACE_PREFIX)
        .filter(|name| !name.is_empty())
}

/// Build the virtual specifier used by completion for a playground module.
pub fn playground_module_specifier(module_name: &str) -> String {
    format!("{MODULE_NAMESPACE_PREFIX}{module_name}")
}

/// Return the text being typed after the last playground namespace prefix.
pub fn playground_completion_module_prefix(source: &str) -> Option<&str> {
    source
        .rsplit_once(MODULE_NAMESPACE_PREFIX)
        .map(|(_, rest)| rest)
}

/// The kind of a completion candidate or symbol, for icon/badge rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Variable,
    Function,
    Method,
    Property,
    Class,
    Module,
    Keyword,
    Global,
    /// A host function exposed via `exposeFunction` / `exposeAsyncFunction`.
    ExposedFn,
}

/// One completion candidate.
#[derive(Debug, Clone)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
}

/// A parameter description supplied by the host for a JavaScript function
/// exposed to the VM. JavaScript does not retain TypeScript annotations at
/// runtime, so hosts provide this metadata explicitly when they want rich
/// hover and completion information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFunctionParameter {
    pub name: String,
    pub type_name: String,
}

/// Language-service metadata for a host-exposed function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFunctionInfo {
    pub name: String,
    pub params: Vec<HostFunctionParameter>,
    pub return_type: String,
    pub documentation: Option<String>,
    pub async_fn: bool,
}

impl HostFunctionInfo {
    pub fn global_info(&self) -> GlobalInfo {
        GlobalInfo {
            name: self.name.clone(),
            shape: Shape::Function {
                params: self
                    .params
                    .iter()
                    .map(|parameter| ParameterInfo {
                        name: parameter.name.clone(),
                        shape: metadata_shape_from_string(&parameter.type_name),
                    })
                    .collect(),
                returns: Box::new(metadata_shape_from_string(&self.return_type)),
                async_fn: self.async_fn,
            },
            documentation: self.documentation.clone(),
        }
    }
}

impl HostFunctionInfo {
    pub fn unknown(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            return_type: "unknown".into(),
            documentation: None,
            async_fn: false,
        }
    }

    pub fn signature(&self) -> String {
        let params = if self.params.is_empty() {
            // An explicitly typed zero-argument function is different from
            // an untyped host callback. The latter keeps the conservative
            // `...args` fallback; metadata with a concrete return type (or
            // documentation) can accurately advertise `()`.
            if self.return_type == "unknown" && self.documentation.is_none() {
                "...args".into()
            } else {
                String::new()
            }
        } else {
            self.params
                .iter()
                .map(|param| format!("{}: {}", param.name, param.type_name))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let return_type = if self.async_fn && !self.return_type.starts_with("Promise<") {
            format!("Promise<{}>", self.return_type)
        } else {
            self.return_type.clone()
        };
        format!("({}) => {}", params, return_type)
    }
}

/// Interpret a legacy display type string as a structured shape.
///
/// Unlike `metadata::parse_legacy_shape`, this never fails: legacy host
/// function metadata carries arbitrary descriptive type names (`User`,
/// `Result<User>`) that only ever reach the editor as display text, so an
/// unrecognised name degrades to `Unknown` rather than rejecting the function.
///
/// It is still bounded. The wrapper recursion is driven by attacker-influenced
/// runtime JSON (`src/lsp/server.rs` reads `returns`/`typeName` straight off the
/// socket), so a string like `"string" + "[]".repeat(200_000)` would otherwise
/// recurse once per wrapper and overflow the stack. Past `MAX_SHAPE_DEPTH` the
/// remainder collapses to `Unknown`, matching the depth limit that
/// `parse_globals` enforces on the declarative path.
fn metadata_shape_from_string(value: &str) -> Shape {
    metadata_shape_from_string_at(value, 0)
}

fn metadata_shape_from_string_at(value: &str, depth: usize) -> Shape {
    let value = value.trim();
    if depth >= metadata::MAX_SHAPE_DEPTH {
        return Shape::Unknown;
    }
    if let Some(inner) = value.strip_suffix("[]") {
        return Shape::Array(Box::new(metadata_shape_from_string_at(inner, depth + 1)));
    }
    if let Some(inner) = value
        .strip_prefix("Promise<")
        .and_then(|value| value.strip_suffix('>'))
    {
        return Shape::Promise(Box::new(metadata_shape_from_string_at(inner, depth + 1)));
    }
    match value {
        "any" => Shape::Any,
        "void" => Shape::Void,
        "undefined" => Shape::Undefined,
        "null" => Shape::Null,
        "boolean" => Shape::Boolean,
        "number" => Shape::Number,
        "string" => Shape::String,
        "object" => Shape::Object(std::collections::BTreeMap::new()),
        "function" => Shape::Function {
            params: Vec::new(),
            returns: Box::new(Shape::Unknown),
            async_fn: false,
        },
        _ => Shape::Unknown,
    }
}

/// Runtime/workspace knowledge the static analyzer cannot derive from source
/// alone. The host fills this in: the playground knows which functions it
/// exposed and which modules it registered.
#[derive(Debug, Clone, Default)]
pub struct AnalysisContext {
    /// Functions the live runtime exposed to the VM as callable globals,
    /// including optional parameter, return-type, and documentation metadata.
    pub exposed_functions: Vec<HostFunctionInfo>,
    /// Host functions declared by the static project manifest. Kept separate
    /// from `exposed_functions` so a stale manifest entry never outranks what
    /// the running program actually exposed.
    pub manifest_functions: Vec<HostFunctionInfo>,
    /// Registered modules and their export names.
    pub modules: Vec<ModuleInfo>,
    /// Shapes observed for VM event handlers. The key is the handler function
    /// name and the value is assigned to its first parameter for completion.
    pub runtime_handlers: HashMap<String, Type>,
    /// Generic globals published by the currently connected runtime.
    pub runtime_globals: HashMap<String, GlobalInfo>,
    /// Generic globals loaded from the optional static LSP manifest.
    pub manifest_globals: HashMap<String, GlobalInfo>,
}

impl AnalysisContext {
    /// Generic globals resolved by the one service-wide precedence order.
    ///
    /// Legacy host functions stay a separate compatibility layer so they keep
    /// their own hover and completion presentation, but they still take part
    /// in precedence: a live runtime function shadows a static manifest global
    /// of the same name, which is why manifest entries are dropped here before
    /// runtime generics are layered on. Callers consult this map first and
    /// `host_functions()` second, yielding:
    ///
    /// ```text
    /// local
    /// > runtime generic
    /// > runtime host function
    /// > manifest generic
    /// > manifest host function
    /// > builtin
    /// ```
    pub(crate) fn resolved_globals(&self) -> HashMap<String, GlobalInfo> {
        let mut globals = self.manifest_globals.clone();
        for function in &self.exposed_functions {
            globals.remove(&function.name);
        }
        globals.extend(self.runtime_globals.clone());
        globals
    }

    /// Host functions in precedence order: live runtime first, then any
    /// manifest declaration the runtime has not superseded.
    pub(crate) fn host_functions(&self) -> Vec<HostFunctionInfo> {
        let mut functions = self.exposed_functions.clone();
        let live: std::collections::HashSet<&str> =
            functions.iter().map(|f| f.name.as_str()).collect();
        let inherited: Vec<_> = self
            .manifest_functions
            .iter()
            .filter(|function| !live.contains(function.name.as_str()))
            .cloned()
            .collect();
        functions.extend(inherited);
        functions
    }
}

/// A registered module: its specifier and the names it exports.
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub name: String,
    pub exports: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Hint,
}

/// A single diagnostic, positioned by 1-based line/column.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub line: usize,
    pub col: usize,
    pub message: String,
    pub severity: DiagnosticSeverity,
}

/// A top-level document symbol.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
}

/// Document symbols for a source program.
pub fn symbols(source: &str) -> Vec<Symbol> {
    let toks = Lexer::new(source).tokenize_with_spans();
    let mut parser = Parser::new_with_spans(toks);
    let stmts = parser.parse();
    symbols::symbols(&stmts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(v: &[Completion]) -> Vec<&str> {
        v.iter().map(|c| c.label.as_str()).collect()
    }

    /// Legacy type strings arrive from untrusted runtime JSON, so the wrapper
    /// recursion must be bounded. Unbounded, `"string" + "[]".repeat(200_000)`
    /// recursed once per wrapper and aborted the language server with a stack
    /// overflow — reachable through `HostFunctionInfo::global_info()`, which
    /// hover and member resolution call on every request.
    #[test]
    fn legacy_type_strings_are_depth_bounded() {
        let deep = "string".to_string() + &"[]".repeat(200_000);
        let shape = metadata_shape_from_string(&deep);

        let mut depth = 0;
        let mut cursor = &shape;
        while let Shape::Array(inner) = cursor {
            depth += 1;
            cursor = inner;
        }
        assert_eq!(depth, metadata::MAX_SHAPE_DEPTH);
        assert_eq!(*cursor, Shape::Unknown);

        // Shapes within the limit still round-trip exactly.
        assert_eq!(
            metadata_shape_from_string("string[]"),
            Shape::Array(Box::new(Shape::String))
        );
        assert_eq!(
            metadata_shape_from_string("Promise<number>"),
            Shape::Promise(Box::new(Shape::Number))
        );
    }

    /// Descriptive names the declarative vocabulary does not cover stay usable
    /// as display text rather than being rejected, matching what legacy
    /// `exposeFunction()` metadata has always allowed.
    #[test]
    fn unknown_legacy_type_names_degrade_instead_of_failing() {
        assert_eq!(metadata_shape_from_string("User"), Shape::Unknown);
        assert_eq!(
            metadata_shape_from_string("Result<User>[]"),
            Shape::Array(Box::new(Shape::Unknown))
        );
        // The raw string is what the editor actually renders.
        let function = HostFunctionInfo {
            name: "getUser".into(),
            params: vec![HostFunctionParameter {
                name: "id".into(),
                type_name: "UserId".into(),
            }],
            return_type: "User".into(),
            documentation: None,
            async_fn: false,
        };
        assert_eq!(function.signature(), "(id: UserId) => User");
    }

    #[test]
    fn ident_offers_globals() {
        let r = complete("Ma", 2, &AnalysisContext::default());
        assert!(labels(&r).contains(&"Math"));
    }

    #[test]
    fn ident_offers_scope_decls() {
        // Trailing newline → empty prefix, so every in-scope name is offered.
        let src = "const counter = 1;\nfunction bump() {}\n";
        let r = complete(src, src.len(), &AnalysisContext::default());
        let l = labels(&r);
        assert!(l.contains(&"counter") && l.contains(&"bump"));
    }

    #[test]
    fn ident_offers_exposed_functions() {
        let ctx = AnalysisContext {
            exposed_functions: vec![HostFunctionInfo::unknown("add")],
            ..Default::default()
        };
        let r = complete("ad", 2, &ctx);
        let add = r.iter().find(|c| c.label == "add").expect("add offered");
        assert_eq!(add.kind, CompletionKind::ExposedFn);
    }

    #[test]
    fn exposed_function_metadata_is_available_to_completion() {
        let ctx = AnalysisContext {
            exposed_functions: vec![HostFunctionInfo {
                name: "alert".into(),
                params: vec![HostFunctionParameter {
                    name: "message".into(),
                    type_name: "string".into(),
                }],
                return_type: "void".into(),
                documentation: Some("Displays a message.".into()),
                async_fn: false,
            }],
            ..Default::default()
        };
        let alert = complete("al", 2, &ctx)
            .into_iter()
            .find(|item| item.label == "alert")
            .expect("alert offered");
        assert_eq!(alert.detail.as_deref(), Some("(message: string) => void"));
    }

    #[test]
    fn ident_offers_module_names() {
        let ctx = AnalysisContext {
            modules: vec![ModuleInfo {
                name: "utils".into(),
                exports: vec![],
            }],
            ..Default::default()
        };
        let r = complete("ut", 2, &ctx);
        assert!(labels(&r).contains(&"utils"));
    }

    #[test]
    fn member_builtin_math() {
        let src = "Math.fl";
        let r = complete(src, src.len(), &AnalysisContext::default());
        assert_eq!(labels(&r), vec!["floor"]);
    }

    #[test]
    fn member_generic_global() {
        let src = "ipc.in";
        let ctx = AnalysisContext {
            runtime_globals: std::collections::BTreeMap::from([("ipc".into(), ipc_global())])
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let r = complete(src, src.len(), &ctx);
        let l = labels(&r);
        assert!(l.contains(&"invoke") && l.contains(&"invokeAsync"));
    }

    #[test]
    fn unknown_local_binding_still_shadows_builtin_members() {
        let source = "let Math;\nMath.";
        let r = complete(source, source.len(), &AnalysisContext::default());
        assert!(r.is_empty());
    }

    #[test]
    fn member_global_alias_includes_generic_global() {
        let src = "window.ipc.";
        let ctx = AnalysisContext {
            runtime_globals: std::collections::BTreeMap::from([("ipc".into(), ipc_global())])
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let r = complete(src, src.len(), &ctx);
        let l = labels(&r);
        assert!(l.contains(&"invoke") && l.contains(&"commands"));
    }

    #[test]
    fn ipc_is_absent_without_metadata() {
        let r = complete("ipc.", 4, &AnalysisContext::default());
        assert!(r.is_empty());
    }

    fn ipc_global() -> GlobalInfo {
        GlobalInfo {
            name: "ipc".into(),
            shape: Shape::Object(std::collections::BTreeMap::from([
                (
                    "invoke".into(),
                    PropertyInfo {
                        shape: Shape::Function {
                            params: vec![ParameterInfo {
                                name: "command".into(),
                                shape: Shape::String,
                            }],
                            returns: Box::new(Shape::Unknown),
                            async_fn: false,
                        },
                        documentation: Some("Invoke a command.".into()),
                    },
                ),
                (
                    "invokeAsync".into(),
                    PropertyInfo {
                        shape: Shape::Function {
                            params: vec![],
                            returns: Box::new(Shape::Unknown),
                            async_fn: true,
                        },
                        documentation: None,
                    },
                ),
                (
                    "commands".into(),
                    PropertyInfo {
                        shape: Shape::Function {
                            params: vec![],
                            returns: Box::new(Shape::Array(Box::new(Shape::String))),
                            async_fn: false,
                        },
                        documentation: None,
                    },
                ),
            ])),
            documentation: None,
        }
    }

    #[test]
    fn member_global_alias_includes_exposed_host_functions() {
        let ctx = AnalysisContext {
            exposed_functions: vec![HostFunctionInfo::unknown("hostNow")],
            ..Default::default()
        };
        let src = "window.host";
        let r = complete(src, src.len(), &ctx);
        assert!(labels(&r).contains(&"hostNow"));
    }

    #[test]
    fn member_object_literal_keys() {
        let src = "const user = { name: 1, age: 2, greet() {} };\nuser.";
        let r = complete(src, src.len(), &AnalysisContext::default());
        let l = labels(&r);
        assert!(l.contains(&"name") && l.contains(&"age") && l.contains(&"greet"));
    }

    #[test]
    fn member_array_literal() {
        let src = "[1, 2, 3].fi";
        let r = complete(src, src.len(), &AnalysisContext::default());
        let l = labels(&r);
        assert!(l.contains(&"filter") && l.contains(&"find"));
    }

    #[test]
    fn member_array_var() {
        let src = "const xs = [1, 2];\nxs.ma";
        let r = complete(src, src.len(), &AnalysisContext::default());
        assert!(labels(&r).contains(&"map"));
    }

    #[test]
    fn member_string_literal() {
        let src = "\"abc\".to";
        let r = complete(src, src.len(), &AnalysisContext::default());
        assert!(labels(&r).contains(&"toUpperCase"));
    }

    #[test]
    fn member_namespace_import_exports() {
        let src = "import * as u from 'utils';\nu.";
        let ctx = AnalysisContext {
            modules: vec![ModuleInfo {
                name: "utils".into(),
                exports: vec!["format".into(), "parse".into()],
            }],
            ..Default::default()
        };
        let r = complete(src, src.len(), &ctx);
        let l = labels(&r);
        assert!(l.contains(&"format") && l.contains(&"parse"));
    }

    #[test]
    fn diagnostics_balanced_ok() {
        assert!(diagnose("const x = [1, 2, { a: (3) }];").is_empty());
    }

    #[test]
    fn diagnostics_unbalanced() {
        assert!(!diagnose("const x = [1, 2;").is_empty());
    }

    #[test]
    fn diagnostics_template_balances() {
        let d = diagnose("const s = `a${x}b`;");
        assert!(d.is_empty(), "template should balance: {:?}", d);
    }

    #[test]
    fn symbols_top_level() {
        let s = symbols("function f() {} class C {} const x = 1;");
        let names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"f") && names.contains(&"C") && names.contains(&"x"));
    }

    #[test]
    fn ident_playground_prefix() {
        let ctx = AnalysisContext {
            modules: vec![
                ModuleInfo {
                    name: "utils".into(),
                    exports: vec!["format".into(), "parse".into()],
                },
                ModuleInfo {
                    name: "math".into(),
                    exports: vec!["PI".into()],
                },
            ],
            ..Default::default()
        };
        let source = format!("{MODULE_NAMESPACE_PREFIX}u");
        let r = complete(&source, source.len(), &ctx);
        let labels: Vec<&str> = r.iter().map(|c| c.label.as_str()).collect();
        let expected = format!("{MODULE_NAMESPACE_PREFIX}utils");
        assert!(labels.contains(&expected.as_str()));
    }

    #[test]
    fn member_playground_namespace_exports() {
        let ctx = AnalysisContext {
            modules: vec![ModuleInfo {
                name: "utils".into(),
                exports: vec!["format".into(), "parse".into()],
            }],
            ..Default::default()
        };
        let source = format!("{MODULE_NAMESPACE_PREFIX}utils.");
        let r = complete(&source, source.len(), &ctx);
        let labels: Vec<&str> = r.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"format") && labels.contains(&"parse"));
    }
}
