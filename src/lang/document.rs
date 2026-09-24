//! A small compiler-style document model shared by all editor frontends.
//!
//! The model is intentionally conservative: unknown values stay `unknown`,
//! while literal objects, functions, promises, arrays, and imports retain the
//! information needed by hover and completion. It never executes guest code.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::lexer::Token;
use crate::parser::{
    BinOp, ClassMember, Expr, ExprOrBlock, MemberName, ObjectProp, Statement, VarKind,
};
use crate::span::SpannedToken;

use super::catalog::{self, BuiltinType};
use super::{GlobalInfo, HostFunctionInfo, ParameterInfo, Shape};

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Any,
    Unknown,
    Void,
    Number,
    String,
    Boolean,
    Null,
    Undefined,
    Object(BTreeMap<String, Type>),
    Class {
        name: String,
        fields: BTreeMap<String, Type>,
        constructor: Vec<String>,
    },
    Instance {
        name: String,
        fields: BTreeMap<String, Type>,
    },
    NativeObject(&'static str),
    Array(Box<Type>),
    Promise(Box<Type>),
    Function {
        params: Vec<TypeParameter>,
        result: Box<Type>,
        async_fn: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeParameter {
    pub name: String,
    pub ty: Type,
    pub show_type: bool,
}

impl TypeParameter {
    fn untyped(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ty: Type::Any,
            show_type: false,
        }
    }

    fn typed(parameter: &ParameterInfo) -> Self {
        Self {
            name: parameter.name.clone(),
            ty: Type::from_shape(&parameter.shape),
            show_type: true,
        }
    }
}

fn untyped_parameters(params: &[String]) -> Vec<TypeParameter> {
    params
        .iter()
        .map(|param| TypeParameter::untyped(param))
        .collect()
}

impl Type {
    pub(crate) fn property(&self, name: &str) -> Type {
        match self {
            Type::Object(fields) => fields.get(name).cloned().unwrap_or(Type::Unknown),
            Type::Class { fields, .. } | Type::Instance { fields, .. } => {
                fields.get(name).cloned().unwrap_or(Type::Unknown)
            }
            Type::NativeObject(receiver) => catalog::builtin_member_type(receiver, name)
                .map(Type::from_builtin)
                .unwrap_or(Type::Unknown),
            Type::Array(_) => catalog::prototype_member_type(catalog::ProtoKind::Array, name)
                .map(Type::from_builtin)
                .unwrap_or(Type::Unknown),
            Type::Promise(value) if matches!(name, "then" | "catch" | "finally") => {
                Type::Function {
                    params: vec![],
                    result: Box::new(Type::Promise(value.clone())),
                    async_fn: false,
                }
            }
            Type::String => catalog::prototype_member_type(catalog::ProtoKind::String, name)
                .map(Type::from_builtin)
                .unwrap_or(Type::Unknown),
            Type::Number => catalog::prototype_member_type(catalog::ProtoKind::Number, name)
                .map(Type::from_builtin)
                .unwrap_or(Type::Unknown),
            Type::Function { .. } if name == "name" => Type::String,
            _ => Type::Unknown,
        }
    }

    pub(crate) fn from_shape(shape: &Shape) -> Self {
        match shape {
            Shape::Unknown => Type::Unknown,
            Shape::Any => Type::Any,
            Shape::Void => Type::Void,
            Shape::Undefined => Type::Undefined,
            Shape::Null => Type::Null,
            Shape::Boolean => Type::Boolean,
            Shape::Number => Type::Number,
            Shape::String => Type::String,
            Shape::Array(item) => Type::Array(Box::new(Self::from_shape(item))),
            Shape::Promise(value) => Type::Promise(Box::new(Self::from_shape(value))),
            Shape::Object(properties) => Type::Object(
                properties
                    .iter()
                    .map(|(name, property)| (name.clone(), Self::from_shape(&property.shape)))
                    .collect(),
            ),
            Shape::Function {
                params,
                returns,
                async_fn,
            } => {
                let result = Self::from_shape(returns);
                let result = if *async_fn && !matches!(result, Type::Promise(_)) {
                    Type::Promise(Box::new(result))
                } else {
                    result
                };
                Type::Function {
                    params: params.iter().map(TypeParameter::typed).collect(),
                    result: Box::new(result),
                    async_fn: *async_fn,
                }
            }
        }
    }

    fn unwrap_promise(&self) -> Type {
        match self {
            Type::Promise(value) => value.as_ref().clone(),
            other => other.clone(),
        }
    }

    pub(crate) fn display(&self) -> String {
        match self {
            Type::Any => "any".into(),
            Type::Unknown => "unknown".into(),
            Type::Void => "void".into(),
            Type::Number => "number".into(),
            Type::String => "string".into(),
            Type::Boolean => "boolean".into(),
            Type::Null => "null".into(),
            Type::Undefined => "undefined".into(),
            Type::Array(item) => format!("{}[]", item.display()),
            Type::Promise(value) => format!("Promise<{}>", value.display()),
            Type::Function {
                params,
                result,
                async_fn,
            } => {
                let params = params
                    .iter()
                    .map(|parameter| {
                        if parameter.show_type {
                            format!("{}: {}", parameter.name, parameter.ty.display())
                        } else {
                            parameter.name.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let result = if *async_fn && !matches!(result.as_ref(), Type::Promise(_)) {
                    format!("Promise<{}>", result.display())
                } else {
                    result.display()
                };
                format!("({params}) => {result}")
            }
            Type::Object(fields) => {
                if fields.is_empty() {
                    return "object".into();
                }
                let body = fields
                    .iter()
                    .map(|(name, ty)| format!("  {}: {};", name, ty.display()))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("{{\n{}\n}}", body)
            }
            Type::Class { name, .. } | Type::Instance { name, .. } => name.clone(),
            Type::NativeObject(name) => (*name).into(),
        }
    }

    pub(crate) fn display_compact(&self) -> String {
        self.display_compact_depth(0)
    }

    fn display_compact_depth(&self, depth: usize) -> String {
        if depth >= 3 {
            return match self {
                Type::Object(_) => "object".into(),
                Type::Array(_) => "unknown[]".into(),
                _ => self.display(),
            };
        }
        match self {
            Type::Object(fields) if fields.is_empty() => "object".into(),
            Type::Object(fields) => {
                let mut parts: Vec<String> = fields
                    .iter()
                    .take(12)
                    .map(|(name, ty)| format!("{}: {}", name, ty.display_compact_depth(depth + 1)))
                    .collect();
                if fields.len() > parts.len() {
                    parts.push("…".into());
                }
                format!("{{ {} }}", parts.join("; "))
            }
            Type::Array(item) => format!("{}[]", item.display_compact_depth(depth + 1)),
            Type::Promise(value) => {
                format!("Promise<{}>", value.display_compact_depth(depth + 1))
            }
            Type::Function {
                params,
                result,
                async_fn,
            } => {
                let params = params
                    .iter()
                    .map(|parameter| {
                        if parameter.show_type {
                            format!(
                                "{}: {}",
                                parameter.name,
                                parameter.ty.display_compact_depth(depth + 1)
                            )
                        } else {
                            parameter.name.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let result = if *async_fn && !matches!(result.as_ref(), Type::Promise(_)) {
                    format!("Promise<{}>", result.display_compact_depth(depth + 1))
                } else {
                    result.display_compact_depth(depth + 1)
                };
                format!("({params}) => {result}")
            }
            _ => self.display(),
        }
    }

    pub(crate) fn from_builtin(builtin: BuiltinType) -> Self {
        match builtin {
            BuiltinType::Unknown => Type::Unknown,
            BuiltinType::Number => Type::Number,
            BuiltinType::String => Type::String,
            BuiltinType::Boolean => Type::Boolean,
            BuiltinType::Undefined => Type::Undefined,
            BuiltinType::Function { result } => Type::Function {
                params: vec![],
                result: Box::new(match result {
                    "number" => Type::Number,
                    "string" => Type::String,
                    "boolean" => Type::Boolean,
                    _ => Type::Unknown,
                }),
                async_fn: false,
            },
            BuiltinType::NativeObject(name) => Type::NativeObject(name),
        }
    }
}

#[derive(Debug, Clone)]
struct Binding {
    kind: String,
    ty: Type,
}

#[derive(Debug, Clone)]
pub struct HoverInfo {
    pub detail: String,
    pub documentation: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedType {
    pub ty: Type,
    pub documentation: Option<String>,
    shape: Option<Shape>,
}

#[derive(Debug, Clone)]
pub struct Document {
    source: String,
    tokens: Vec<SpannedToken>,
    bindings: HashMap<String, Binding>,
    properties: HashMap<String, Type>,
    exports: HashMap<String, Type>,
    host_functions: HashMap<String, HostFunctionInfo>,
    globals: HashMap<String, GlobalInfo>,
    parameter_values: HashMap<String, String>,
}

impl Document {
    pub fn parse(source: &str) -> Self {
        Self::parse_with_modules(source, &HashMap::new())
    }

    pub fn parse_with_modules(source: &str, module_sources: &HashMap<String, String>) -> Self {
        Self::parse_with_context(source, module_sources, &[])
    }

    pub fn parse_with_context(
        source: &str,
        module_sources: &HashMap<String, String>,
        host_functions: &[HostFunctionInfo],
    ) -> Self {
        Self::parse_with_context_and_runtime(
            source,
            module_sources,
            host_functions,
            &HashMap::new(),
            &HashMap::new(),
        )
    }

    pub fn parse_with_context_and_runtime(
        source: &str,
        module_sources: &HashMap<String, String>,
        host_functions: &[HostFunctionInfo],
        runtime_handlers: &HashMap<String, Type>,
        runtime_values: &HashMap<String, String>,
    ) -> Self {
        Self::parse_with_context_and_runtime_and_globals(
            source,
            module_sources,
            host_functions,
            runtime_handlers,
            runtime_values,
            &HashMap::new(),
        )
    }

    pub fn parse_with_context_and_runtime_and_globals(
        source: &str,
        module_sources: &HashMap<String, String>,
        host_functions: &[HostFunctionInfo],
        runtime_handlers: &HashMap<String, Type>,
        runtime_values: &HashMap<String, String>,
        globals: &HashMap<String, GlobalInfo>,
    ) -> Self {
        let mut module_stack = HashSet::new();
        Self::parse_with_context_inner(
            source,
            module_sources,
            host_functions,
            runtime_handlers,
            runtime_values,
            globals,
            &mut module_stack,
        )
    }

    fn parse_with_context_inner(
        source: &str,
        module_sources: &HashMap<String, String>,
        host_functions: &[HostFunctionInfo],
        runtime_handlers: &HashMap<String, Type>,
        runtime_values: &HashMap<String, String>,
        globals: &HashMap<String, GlobalInfo>,
        module_stack: &mut HashSet<String>,
    ) -> Self {
        let mut lexer = crate::lexer::Lexer::new(source);
        let tokens = lexer.tokenize_with_spans();
        let mut parser = crate::parser::Parser::new_with_spans(tokens.clone());
        let statements = parser.parse();
        let mut builder = Builder {
            bindings: HashMap::new(),
            properties: HashMap::new(),
            exports: HashMap::new(),
            module_sources,
            host_functions,
            runtime_handlers,
            runtime_values,
            globals,
            parameter_values: HashMap::new(),
            module_stack,
        };
        builder.statements(&statements, &HashMap::new());
        Self {
            source: source.to_string(),
            tokens,
            bindings: builder.bindings,
            properties: builder.properties,
            exports: builder.exports,
            host_functions: host_functions
                .iter()
                .cloned()
                .map(|function| (function.name.clone(), function))
                .collect(),
            globals: globals.clone(),
            parameter_values: builder.parameter_values,
        }
    }

    pub fn hover(&self, offset: usize) -> Option<HoverInfo> {
        let token_index = self.token_at(offset)?;
        let (token, _) = &self.tokens[token_index];
        let name = match token {
            Token::Identifier(name) => name,
            _ => return None,
        };

        if token_index >= 2
            && matches!(
                self.tokens[token_index - 1].0,
                Token::Dot | Token::QuestionDot
            )
        {
            let receiver = self.receiver_name(token_index - 2);
            let resolved = receiver
                .as_deref()
                .and_then(|receiver| self.resolve_path(&format!("{receiver}.{name}")));
            let ty = resolved
                .as_ref()
                .map(|resolved| resolved.ty.clone())
                .or_else(|| self.properties.get(name).cloned())
                .unwrap_or(Type::Unknown);
            return Some(HoverInfo {
                detail: format!("(property) {}: {}", name, ty.display_compact()),
                documentation: resolved.and_then(|resolved| resolved.documentation),
            });
        }

        let binding = self.bindings.get(name);
        if let Some(binding) = binding {
            let detail = match &binding.kind[..] {
                "function" => format!("(function) {}: {}", name, binding.ty.display_compact()),
                "parameter" => format!("(parameter) {}: {}", name, binding.ty.display_compact()),
                "import" => format!("(import) {}: {}", name, binding.ty.display_compact()),
                "class" => format!("(class) {}: {}", name, binding.ty.display_compact()),
                kind => format!("{} {}: {}", kind, name, binding.ty.display_compact()),
            };
            return Some(HoverInfo {
                detail,
                documentation: self
                    .parameter_values
                    .get(name)
                    .map(|value| format!("Last value:\n```json\n{}\n```", value)),
            });
        }

        if !self.globals.contains_key(name)
            && let Some(function) = self.host_functions.get(name)
        {
            return Some(HoverInfo {
                detail: format!("(function) {}: {}", name, function.signature()),
                documentation: function.documentation.clone(),
            });
        }

        if let Some(resolved) = self.resolve_path(name) {
            return Some(HoverInfo {
                detail: format!("(global) {}: {}", name, resolved.ty.display()),
                documentation: resolved.documentation,
            });
        }

        let builtin = catalog::builtin_global_type(name).map(Type::from_builtin)?;
        Some(HoverInfo {
            detail: format!("(global) {}: {}", name, builtin.display()),
            documentation: None,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn export_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.exports.keys().cloned().collect();
        names.sort();
        names
    }

    fn token_at(&self, offset: usize) -> Option<usize> {
        let mut offset = offset.min(self.source.len());
        while offset > 0 && !self.source.is_char_boundary(offset) {
            offset -= 1;
        }
        let (line, col) = position_at(&self.source, offset);
        self.tokens
            .iter()
            .enumerate()
            .find_map(|(index, (token, span))| {
                let Token::Identifier(name) = token else {
                    return None;
                };
                if span.line == line && col >= span.col && col <= span.col + name.chars().count() {
                    Some(index)
                } else {
                    None
                }
            })
    }

    fn receiver_name(&self, index: usize) -> Option<String> {
        let mut parts = Vec::new();
        let mut cursor = index;
        let Token::Identifier(name) = self.tokens.get(cursor)?.0.clone() else {
            return None;
        };
        parts.push(name);
        while cursor >= 2
            && matches!(
                self.tokens.get(cursor - 1).map(|token| &token.0),
                Some(Token::Dot | Token::QuestionDot)
            )
        {
            cursor -= 2;
            let Token::Identifier(name) = self.tokens.get(cursor)?.0.clone() else {
                break;
            };
            parts.push(name);
        }
        parts.reverse();
        Some(parts.join("."))
    }

    pub(crate) fn resolve_path(&self, path: &str) -> Option<ResolvedType> {
        let mut segments = path.split('.');
        let first = segments.next()?.trim();
        if first.is_empty() {
            return None;
        }

        if let Some(binding) = self.bindings.get(first) {
            let mut resolved = ResolvedType {
                ty: binding.ty.clone(),
                documentation: None,
                shape: None,
            };
            for segment in segments {
                resolved.ty = resolved.ty.property(segment);
                resolved.documentation = None;
            }
            return Some(resolved);
        }

        // An explicit global-object alias addresses the VM's global object,
        // not a same-named lexical binding in the current source. Bare
        // `ipc` still takes the binding branch above, while `window.ipc` and
        // `globalThis.ipc` resolve the published runtime/manifest global.
        if matches!(first, "window" | "globalThis" | "self") {
            let Some(global_name) = segments.next() else {
                return Some(ResolvedType {
                    ty: Type::Unknown,
                    documentation: None,
                    shape: None,
                });
            };
            let suffix = std::iter::once(global_name).chain(segments);
            let path = suffix.collect::<Vec<_>>().join(".");
            return self.resolve_global_path(&path);
        }

        self.resolve_global_path(path)
    }

    pub(crate) fn has_binding(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }

    fn resolve_global_path(&self, path: &str) -> Option<ResolvedType> {
        let mut segments = path.split('.');
        let first = segments.next()?.trim();
        if first.is_empty() {
            return None;
        }

        if let Some(global) = self.globals.get(first) {
            let mut resolved = ResolvedType {
                ty: Type::from_shape(&global.shape),
                documentation: global.documentation.clone(),
                shape: Some(global.shape.clone()),
            };
            for segment in segments {
                resolved = property_resolution(resolved, segment);
            }
            return Some(resolved);
        }

        if let Some(function) = self.host_functions.get(first) {
            let global = function.global_info();
            let mut resolved = ResolvedType {
                ty: Type::from_shape(&global.shape),
                documentation: global.documentation,
                shape: Some(global.shape.clone()),
            };
            for segment in segments {
                resolved = property_resolution(resolved, segment);
            }
            return Some(resolved);
        }

        let mut resolved = ResolvedType {
            ty: Type::from_builtin(catalog::builtin_global_type(first)?),
            documentation: None,
            shape: None,
        };
        for segment in segments {
            resolved = property_resolution(resolved, segment);
        }
        Some(resolved)
    }
}

fn property_resolution(mut resolved: ResolvedType, segment: &str) -> ResolvedType {
    resolved.ty = resolved.ty.property(segment);
    let next_shape = resolved.shape.take().and_then(|shape| {
        let property = shape.property(segment)?;
        resolved.documentation = property.documentation.clone();
        Some(property.shape.clone())
    });
    resolved.shape = next_shape;
    if resolved.shape.is_none() {
        resolved.documentation = None;
    }
    resolved
}

struct Builder<'a> {
    bindings: HashMap<String, Binding>,
    properties: HashMap<String, Type>,
    exports: HashMap<String, Type>,
    module_sources: &'a HashMap<String, String>,
    host_functions: &'a [HostFunctionInfo],
    runtime_handlers: &'a HashMap<String, Type>,
    runtime_values: &'a HashMap<String, String>,
    globals: &'a HashMap<String, GlobalInfo>,
    parameter_values: HashMap<String, String>,
    module_stack: &'a mut HashSet<String>,
}

impl Builder<'_> {
    fn statements(&mut self, statements: &[Statement], outer: &HashMap<String, Type>) {
        let mut env = outer.clone();
        for statement in statements {
            self.statement(statement, &mut env);
        }
    }

    fn statement(&mut self, statement: &Statement, env: &mut HashMap<String, Type>) {
        match statement {
            Statement::VarDecl {
                kind, name, init, ..
            } => {
                let ty = init
                    .as_deref()
                    .map(|expr| self.expr(expr, env))
                    .unwrap_or(Type::Unknown);
                let kind = match kind {
                    VarKind::Var => "var",
                    VarKind::Let => "let",
                    VarKind::Const => "const",
                };
                self.bindings.insert(
                    name.clone(),
                    Binding {
                        kind: kind.into(),
                        ty: ty.clone(),
                    },
                );
                env.insert(name.clone(), ty);
            }
            Statement::FnDecl {
                name,
                params,
                body,
                is_async,
                ..
            } => {
                let result = self.function_result_named(Some(name), params, body, env);
                let ty = Type::Function {
                    params: untyped_parameters(params),
                    result: Box::new(if *is_async {
                        Type::Promise(Box::new(result))
                    } else {
                        result
                    }),
                    async_fn: *is_async,
                };
                self.bindings.insert(
                    name.clone(),
                    Binding {
                        kind: "function".into(),
                        ty: ty.clone(),
                    },
                );
                env.insert(name.clone(), ty);
            }
            Statement::ClassDecl { name, body, .. } => {
                let ty = self.class_type(name, body, env);
                self.bindings.insert(
                    name.clone(),
                    Binding {
                        kind: "class".into(),
                        ty: ty.clone(),
                    },
                );
                env.insert(name.clone(), ty);
            }
            Statement::Import {
                module,
                default,
                named,
                namespace,
            } => {
                let exports = self.module_exports(module);
                if let Some(name) = default {
                    let ty = exports.get("default").cloned().unwrap_or(Type::Unknown);
                    self.bindings.insert(
                        name.clone(),
                        Binding {
                            kind: "import".into(),
                            ty: ty.clone(),
                        },
                    );
                    env.insert(name.clone(), ty);
                }
                for (local, imported) in named {
                    let ty = exports.get(imported).cloned().unwrap_or(Type::Unknown);
                    self.bindings.insert(
                        local.clone(),
                        Binding {
                            kind: "import".into(),
                            ty: ty.clone(),
                        },
                    );
                    env.insert(local.clone(), ty);
                }
                if let Some(namespace) = namespace {
                    let ty = Type::Object(exports.into_iter().collect());
                    self.bindings.insert(
                        namespace.clone(),
                        Binding {
                            kind: "import".into(),
                            ty: ty.clone(),
                        },
                    );
                    env.insert(namespace.clone(), ty);
                }
            }
            Statement::Expr(expr) => {
                self.expr(expr, env);
            }
            Statement::Block(body) => self.statements(body, env),
            // A declarator group — including the `export <declaration>`
            // desugaring — shares the enclosing scope, so its declarations
            // stay visible to what follows: `export class Store {}; export
            // function createStore() { return new Store(); }`.
            Statement::Declarations(body) => {
                for statement in body {
                    self.statement(statement, env);
                }
            }
            Statement::If { then, else_, .. } => {
                self.statements(then, env);
                if let Some(body) = else_ {
                    self.statements(body, env);
                }
            }
            Statement::While { body, .. } | Statement::DoWhile { body, .. } => {
                self.statements(body, env)
            }
            Statement::For { body, .. } => self.statements(body, env),
            Statement::ForIn { body, .. } | Statement::ForOf { body, .. } => {
                self.statements(body, env)
            }
            Statement::Try {
                body,
                catch,
                finally,
            } => {
                self.statements(body, env);
                if let Some((name, body)) = catch {
                    self.bindings.insert(
                        name.clone(),
                        Binding {
                            kind: "parameter".into(),
                            ty: Type::Any,
                        },
                    );
                    self.statements(body, env);
                }
                if let Some(body) = finally {
                    self.statements(body, env);
                }
            }
            Statement::ExportDefault(value) => {
                let ty = self.expr(value, env);
                self.exports.insert("default".into(), ty);
            }
            Statement::ExportNamed { specifiers, source } => {
                let source_exports = source.as_deref().map(|module| self.module_exports(module));
                for (local, exported) in specifiers {
                    let ty = source_exports
                        .as_ref()
                        .and_then(|exports| exports.get(local))
                        .cloned()
                        .or_else(|| env.get(local).cloned())
                        .unwrap_or(Type::Unknown);
                    self.exports.insert(exported.clone(), ty);
                }
            }
            _ => {}
        }
    }

    fn module_exports(&mut self, module: &str) -> HashMap<String, Type> {
        let Some(source) = self.module_sources.get(module).cloned() else {
            return HashMap::new();
        };
        if !self.module_stack.insert(module.to_string()) {
            return HashMap::new();
        }
        let document = Document::parse_with_context_inner(
            &source,
            self.module_sources,
            self.host_functions,
            self.runtime_handlers,
            self.runtime_values,
            self.globals,
            self.module_stack,
        );
        self.module_stack.remove(module);
        document.exports
    }

    fn function_result(
        &mut self,
        params: &[String],
        body: &[Statement],
        outer: &HashMap<String, Type>,
    ) -> Type {
        self.function_result_named(None, params, body, outer)
    }

    fn function_result_named(
        &mut self,
        function_name: Option<&str>,
        params: &[String],
        body: &[Statement],
        outer: &HashMap<String, Type>,
    ) -> Type {
        let mut env = outer.clone();
        for (index, param) in params.iter().enumerate() {
            let runtime_type = (index == 0)
                .then(|| function_name.and_then(|name| self.runtime_handlers.get(name)))
                .flatten()
                .cloned()
                .unwrap_or(Type::Any);
            env.insert(param.clone(), Type::Any);
            if index == 0
                && let Some(name) = function_name
                && let Some(value) = self.runtime_values.get(name)
            {
                self.parameter_values.insert(param.clone(), value.clone());
            }
            self.bindings.insert(
                param.clone(),
                Binding {
                    kind: "parameter".into(),
                    ty: runtime_type.clone(),
                },
            );
            env.insert(param.clone(), runtime_type);
        }
        let mut result = Type::Unknown;
        for statement in body {
            if let Statement::Return(value) = statement {
                result = value
                    .as_deref()
                    .map(|expr| self.expr(expr, &mut env))
                    .unwrap_or(Type::Undefined);
            } else {
                self.statement(statement, &mut env);
            }
        }
        result
    }

    fn expr(&mut self, expr: &Expr, env: &mut HashMap<String, Type>) -> Type {
        match expr {
            Expr::Number(_) => Type::Number,
            Expr::String(_) | Expr::Template { .. } => Type::String,
            // A tag can return anything, so its call site is unconstrained.
            Expr::TaggedTemplate { .. } => Type::Unknown,
            // `import(…)` resolves to a namespace object.
            Expr::DynamicImport(_) => Type::Unknown,
            Expr::ClassExpr { .. } => Type::Unknown,
            Expr::Regex(_, _) | Expr::BigIntLiteral(_) => Type::Unknown,
            // A logical assignment evaluates to either operand.
            Expr::LogicalAssignment { .. } => Type::Unknown,
            Expr::Bool(_) => Type::Boolean,
            Expr::Null => Type::Null,
            Expr::Undefined => Type::Undefined,
            Expr::Identifier(name) => env
                .get(name)
                .cloned()
                .or_else(|| {
                    self.globals
                        .get(name)
                        .map(|global| Type::from_shape(&global.shape))
                })
                .or_else(|| {
                    self.host_functions
                        .iter()
                        .find(|function| function.name == *name)
                        .map(|function| Type::from_shape(&function.global_info().shape))
                })
                .or_else(|| catalog::builtin_global_type(name).map(Type::from_builtin))
                .unwrap_or(Type::Unknown),
            Expr::Object(props) => {
                let mut fields = BTreeMap::new();
                for prop in props {
                    match prop {
                        ObjectProp::Shorthand(name) => {
                            fields.insert(name.clone(), Type::Any);
                        }
                        ObjectProp::KeyValue(name, value) => {
                            let ty = self.expr(value, env);
                            self.properties.insert(name.clone(), ty.clone());
                            fields.insert(name.clone(), ty);
                        }
                        ObjectProp::Method {
                            name, params, body, ..
                        } => {
                            fields.insert(
                                name.clone(),
                                Type::Function {
                                    params: untyped_parameters(params),
                                    result: Box::new(self.function_result(params, body, env)),
                                    async_fn: false,
                                },
                            );
                        }
                        ObjectProp::Getter { name, body } => {
                            fields.insert(name.clone(), self.function_result(&[], body, env));
                        }
                        ObjectProp::Setter { name, param, body } => {
                            fields.insert(
                                name.clone(),
                                Type::Function {
                                    params: vec![TypeParameter::untyped(param)],
                                    result: Box::new(self.function_result(
                                        std::slice::from_ref(param),
                                        body,
                                        env,
                                    )),
                                    async_fn: false,
                                },
                            );
                        }
                        ObjectProp::Computed(_, _) | ObjectProp::Spread(_) => {}
                    }
                }
                Type::Object(fields)
            }
            Expr::Array(items) => Type::Array(Box::new(
                items
                    .first()
                    .map(|item| self.expr(item, env))
                    .unwrap_or(Type::Unknown),
            )),
            Expr::Await(value) => self.expr(value, env).unwrap_promise(),
            Expr::Call { callee, args } => {
                if let Expr::Member {
                    object, property, ..
                } = callee.as_ref()
                    && let Some(method) = expression_property_name(property)
                {
                    let object_ty = self.expr(object, env);
                    if method == "resolve"
                        && matches!(object.as_ref(), Expr::Identifier(name) if name == "Promise")
                    {
                        return Type::Promise(Box::new(
                            args.first()
                                .map(|arg| self.expr(arg, env))
                                .unwrap_or(Type::Undefined),
                        ));
                    }
                    if method == "then"
                        && let Some(Expr::ArrowFn { params, body, .. }) = args.first()
                    {
                        let mut arrow_env = env.clone();
                        let value_ty = object_ty.unwrap_promise();
                        for param in params {
                            arrow_env.insert(param.clone(), value_ty.clone());
                            self.bindings.insert(
                                param.clone(),
                                Binding {
                                    kind: "parameter".into(),
                                    ty: value_ty.clone(),
                                },
                            );
                        }
                        let result = match body.as_ref() {
                            ExprOrBlock::Expr(value) => self.expr(value, &mut arrow_env),
                            ExprOrBlock::Block(body) => {
                                self.function_result(params, body, &arrow_env)
                            }
                        };
                        for param in params {
                            self.bindings.insert(
                                param.clone(),
                                Binding {
                                    kind: "parameter".into(),
                                    ty: value_ty.clone(),
                                },
                            );
                        }
                        return Type::Promise(Box::new(result));
                    }
                }
                match self.expr(callee, env) {
                    Type::Function { result, .. } => result.as_ref().clone(),
                    _ => Type::Unknown,
                }
            }
            Expr::Member {
                object, property, ..
            }
            | Expr::OptionalChain {
                object, property, ..
            } => {
                let object_ty = self.expr(object, env);
                let name = match property.as_ref() {
                    Expr::Identifier(name) | Expr::String(name) => name,
                    _ => return Type::Unknown,
                };
                let property_ty = object_ty.property(name);
                self.properties
                    .entry(name.clone())
                    .or_insert_with(|| property_ty.clone());
                property_ty
            }
            Expr::ArrowFn { params, body, .. } => {
                let result = match body.as_ref() {
                    ExprOrBlock::Expr(value) => self.expr(value, env),
                    ExprOrBlock::Block(body) => self.function_result(params, body, env),
                };
                Type::Function {
                    params: untyped_parameters(params),
                    result: Box::new(result),
                    async_fn: false,
                }
            }
            Expr::FnExpr {
                name: _,
                params,
                body,
                is_async,
                ..
            } => {
                let result = self.function_result(params, body, env);
                Type::Function {
                    params: untyped_parameters(params),
                    result: Box::new(if *is_async {
                        Type::Promise(Box::new(result))
                    } else {
                        result
                    }),
                    async_fn: *is_async,
                }
            }
            Expr::Binary { op, left, right } => {
                let left_ty = self.expr(left, env);
                let right_ty = self.expr(right, env);
                if matches!(op, BinOp::Add) && (left_ty == Type::String || right_ty == Type::String)
                {
                    Type::String
                } else {
                    Type::Number
                }
            }
            Expr::New { callee, .. } => match self.expr(callee, env) {
                Type::Class { name, fields, .. } => Type::Instance { name, fields },
                _ => Type::Unknown,
            },
            Expr::Conditional { consequent, .. } => self.expr(consequent, env),
            Expr::Unary { .. } => Type::Number,
            Expr::Assignment { value, .. } => self.expr(value, env),
            Expr::Spread(value) => self.expr(value, env),
            Expr::This | Expr::Super | Expr::ImportMeta | Expr::Yield(_) | Expr::YieldFrom(_) => {
                Type::Unknown
            }
        }
    }

    fn class_type(
        &mut self,
        name: &str,
        members: &[ClassMember],
        outer: &HashMap<String, Type>,
    ) -> Type {
        let mut fields = BTreeMap::new();
        let mut constructor = Vec::new();

        for member in members {
            match member {
                ClassMember::Method {
                    name: MemberName::Static(member_name),
                    is_static,
                    params,
                    body,
                    ..
                } if !is_static && member_name == "constructor" => {
                    constructor = params.clone();
                    let mut env = outer.clone();
                    for param in params {
                        env.insert(param.clone(), Type::Any);
                    }
                    self.collect_instance_fields(body, &mut env, &mut fields);
                }
                // Computed names contribute no static type entry.
                ClassMember::Method {
                    name: MemberName::Static(member_name),
                    is_static,
                    params,
                    body,
                    ..
                } if !is_static => {
                    let env = outer.clone();
                    let result = self.function_result(params, body, &env);
                    fields.insert(
                        member_name.clone(),
                        Type::Function {
                            params: untyped_parameters(params),
                            result: Box::new(result),
                            async_fn: false,
                        },
                    );
                }
                ClassMember::Field {
                    name: MemberName::Static(field_name),
                    is_static,
                    init,
                } if !is_static => {
                    fields.insert(
                        field_name.clone(),
                        init.as_ref()
                            .map(|value| self.expr(value, &mut outer.clone()))
                            .unwrap_or(Type::Unknown),
                    );
                }
                ClassMember::Getter {
                    name: MemberName::Static(field_name),
                    is_static,
                    body,
                } if !is_static => {
                    fields.insert(field_name.clone(), self.function_result(&[], body, outer));
                }
                ClassMember::Setter {
                    name: MemberName::Static(field_name),
                    is_static,
                    param,
                    body,
                } if !is_static => {
                    fields.insert(
                        field_name.clone(),
                        Type::Function {
                            params: vec![TypeParameter::untyped(param)],
                            result: Box::new(self.function_result(
                                std::slice::from_ref(param),
                                body,
                                outer,
                            )),
                            async_fn: false,
                        },
                    );
                }
                _ => {}
            }
        }

        Type::Class {
            name: name.to_string(),
            fields,
            constructor,
        }
    }

    fn collect_instance_fields(
        &mut self,
        statements: &[Statement],
        env: &mut HashMap<String, Type>,
        fields: &mut BTreeMap<String, Type>,
    ) {
        for statement in statements {
            match statement {
                Statement::Expr(expr) => self.collect_instance_expr(expr, env, fields),
                Statement::If { then, else_, .. } => {
                    self.collect_instance_fields(then, env, fields);
                    if let Some(else_body) = else_ {
                        self.collect_instance_fields(else_body, env, fields);
                    }
                }
                Statement::Block(body)
                | Statement::Declarations(body)
                | Statement::While { body, .. }
                | Statement::DoWhile { body, .. }
                | Statement::For { body, .. }
                | Statement::ForIn { body, .. }
                | Statement::ForOf { body, .. } => {
                    self.collect_instance_fields(body, env, fields);
                }
                _ => self.statement(statement, env),
            }
        }
    }

    fn collect_instance_expr(
        &mut self,
        expr: &Expr,
        env: &mut HashMap<String, Type>,
        fields: &mut BTreeMap<String, Type>,
    ) {
        if let Expr::Assignment { target, value, .. } = expr
            && let Expr::Member {
                object, property, ..
            } = target.as_ref()
            && matches!(object.as_ref(), Expr::This)
            && let Some(name) = expression_property_name(property)
        {
            fields.insert(name.to_string(), self.expr(value, env));
        }
        self.expr(expr, env);
    }
}

fn position_at(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for ch in source[..offset].chars() {
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn expression_property_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Identifier(name) | Expr::String(name) => Some(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hover(source: &str, word: &str) -> String {
        let offset = source.rfind(word).unwrap() + 1;
        Document::parse(source)
            .hover(offset)
            .unwrap_or_else(|| panic!("no hover for {word} at {offset}"))
            .detail
    }

    #[test]
    fn infers_literal_variable() {
        assert_eq!(hover("let total = 0; total;", "total"), "let total: number");
    }

    #[test]
    fn infers_promise_callback_and_property() {
        let source = "async function loadUser(id) { const response = await Promise.resolve({ id, name: \"Ada\" }); return response; } loadUser(42).then((user) => { user.name; });";
        let user_hover = hover(source, "user");
        assert!(user_hover.contains("(parameter) user: {"));
        assert_eq!(hover(source, "name"), "(property) name: string");
    }

    #[test]
    fn preserves_class_instance_type_through_factory() {
        let source = r#"
            class Store {
                constructor(initial) {
                    this.state = initial;
                    this.listeners = [];
                }
                read(key) { return this.state[key]; }
            }
            function createStore(initial) { return new Store(initial); }
            const store = createStore({ count: 0 });
            store.read("count");
            store.state;
        "#;

        let class_offset = source.find("class Store").unwrap() + "class ".len() + 1;
        assert_eq!(
            Document::parse(source).hover(class_offset).unwrap().detail,
            "(class) Store: Store"
        );
        assert!(hover(source, "createStore").contains("Store"));
        assert_eq!(hover(source, "store"), "const store: Store");
        assert!(hover(source, "read").starts_with("(property) read:"));
        assert_eq!(hover(source, "state"), "(property) state: any");
    }

    #[test]
    fn injects_native_date_types_from_catalog() {
        let source = "const start = Date.now(); start; Date.parse(\"2024-01-01\");";
        assert_eq!(hover(source, "now"), "(property) now: () => number");
        assert_eq!(hover(source, "parse"), "(property) parse: () => number");
        assert_eq!(hover(source, "start"), "const start: number");
    }

    #[test]
    fn infers_the_real_store_module_factory() {
        let source = include_str!("../../playground/public/examples/modules/store.js");
        let detail = hover(source, "createStore");
        assert_eq!(detail, "(function) createStore: (initial) => Store");
    }

    #[test]
    fn hovers_host_function_metadata_and_documentation() {
        let host_functions = [HostFunctionInfo {
            name: "alert".into(),
            params: vec![super::super::HostFunctionParameter {
                name: "message".into(),
                type_name: "string".into(),
            }],
            return_type: "void".into(),
            documentation: Some("Displays a message in the playground.".into()),
            async_fn: false,
        }];
        let document =
            Document::parse_with_context("alert(\"hello\");", &HashMap::new(), &host_functions);
        let info = document
            .hover(2)
            .expect("host function should have hover information");
        assert_eq!(info.detail, "(function) alert: (message: string) => void");
        assert_eq!(
            info.documentation.as_deref(),
            Some("Displays a message in the playground.")
        );
    }

    #[test]
    fn infers_string_prototype_return_types() {
        let source = include_str!("../../playground/public/examples/modules/format.js");
        assert_eq!(hover(source, "upper"), "(function) upper: (s) => string");
        assert_eq!(
            hover(source, "toUpperCase"),
            "(property) toUpperCase: () => string"
        );
    }

    #[test]
    fn propagates_types_across_imported_modules() {
        let mut modules = HashMap::new();
        modules.insert(
            "./modules/store.js".into(),
            include_str!("../../playground/public/examples/modules/store.js").into(),
        );
        let source = "import createStore from \"./modules/store.js\"; const store = createStore({ count: 0 }); store;";
        let document = Document::parse_with_modules(source, &modules);
        let offset = source.find("createStore").unwrap() + 2;
        assert_eq!(
            document.hover(offset).unwrap().detail,
            "(import) createStore: (initial) => Store"
        );
    }

    #[test]
    fn infers_exported_class_factory() {
        let source =
            "export class Store {} export function createStore(value) { return new Store(value); }";
        assert_eq!(
            hover(source, "createStore"),
            "(function) createStore: (value) => Store"
        );
    }

    #[test]
    fn hover_never_panics_on_non_ascii_cursor_offset() {
        let source = "// 🦀\nconst total = 1; total;";
        // This deliberately points into the four-byte emoji.
        assert!(Document::parse(source).hover(4).is_none());
    }

    #[test]
    fn hover_never_panics_on_a_member_without_a_receiver() {
        let document = Document::parse(".name");
        let _ = document.hover(".name".len());
    }
}
