//! Module graph instantiation precedes every body. Export resolution walks the
//! graph without executing dependencies; evaluation then follows dependency order.
mod asynchronous;
use super::*;
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct ModuleGraph {
    records: HashMap<String, LinkedModule>,
}
struct LinkedModule {
    program: PreparedProgram,
    requests: Vec<(String, String)>,
    state: ModuleState,
    evaluation: Option<Rc<RefCell<crate::value::PromiseInner>>>,
}
#[derive(Clone)]
enum ModuleState {
    Linked,
    Evaluating,
    Evaluated,
    Failed(Value),
}
impl ModuleGraph {
    pub(super) fn trace_values(&self) -> Vec<Value> {
        let mut values = Vec::new();
        for record in self.records.values() {
            if let ModuleState::Failed(value) = &record.state {
                values.push(value.clone());
            }
            values.extend(record.evaluation.iter().cloned().map(Value::Promise));
        }
        values
    }
    pub(super) fn remove(&mut self, name: &str) {
        self.records.remove(name);
    }
}
fn flat(stmts: &[Statement]) -> Vec<&Statement> {
    let mut out = Vec::new();
    for stmt in stmts {
        if let Statement::Declarations(inner) = stmt {
            out.extend(flat(inner));
        } else {
            out.push(stmt);
        }
    }
    out
}
fn linked_body(stmts: &[Statement]) -> Vec<Statement> {
    stmts
        .iter()
        .map(|stmt| match stmt {
            Statement::FnDecl { .. } => Statement::Empty,
            Statement::ExportDefault(expr)
                if matches!(expr.as_ref(), crate::parser::Expr::FnExpr { .. }) =>
            {
                Statement::Empty
            }
            Statement::Declarations(inner) => Statement::Declarations(linked_body(inner)),
            _ => stmt.clone(),
        })
        .collect()
}
fn default_local(stmts: &[Statement], default: &Statement) -> Option<String> {
    for stmt in stmts {
        if let Statement::Declarations(inner) = stmt {
            if inner.iter().any(|stmt| std::ptr::eq(stmt, default)) {
                return inner.iter().find_map(|stmt| match stmt {
                    Statement::FnDecl { name, .. } | Statement::ClassDecl { name, .. } => {
                        Some(name.clone())
                    }
                    _ => None,
                });
            }
            if let Some(name) = default_local(inner, default) {
                return Some(name);
            }
        }
    }
    None
}
fn syntax(message: impl Into<String>) -> VmErr {
    VmErr::Msg(format!("SyntaxError: {}", message.into()))
}
fn error_value(error: &VmErr) -> Value {
    match error {
        VmErr::Throw(value) => value.clone(),
        _ => crate::error::error_value_from_msg(&error.to_string()),
    }
}
impl Interpreter {
    /// Instantiate and validate a module graph without running module bodies.
    /// The argument is a canonical loader identifier (as returned by resolve).
    pub fn link_module(&mut self, name: &str) -> Result<bool, VmErr> {
        if self.module_graph.borrow().records.contains_key(name) {
            return Ok(true);
        }
        let depth = self.guest_execution_depth.clone();
        depth.set(depth.get().saturating_add(1));
        let _guard = GuestExecutionGuard(depth);
        let mut created = Vec::new();
        let result = (|| {
            if !self.discover_module(name, &mut created, 0)? {
                return Ok(false);
            }
            // Resolve stars and indirect exports after the whole graph exists.
            for _ in 0..2 {
                for id in &created {
                    let names = self.exported_names(id, &mut HashSet::new());
                    for export in names {
                        if let Some(value) =
                            self.resolve_export(id, &export, &mut HashSet::new())?
                        {
                            let mut modules = self.modules.borrow_mut();
                            let record = modules.get_mut(id).expect("instantiated module");
                            if export == "default" {
                                record.default = Some(value);
                            } else {
                                record.exports.insert(export, value);
                            }
                        }
                    }
                }
            }
            // Namespace re-exports may have created placeholders before all
            // indirect exports were installed. Complete those same objects.
            for id in &created {
                let module = self.module(id).expect("linked module");
                let cached = module.namespace.borrow_mut().take();
                if let Some(Value::Object { props: original }) = &cached {
                    let completed_namespace = Self::namespace_object(&module)?;
                    let Value::Object { props: completed } = &completed_namespace else {
                        unreachable!("namespace is an object");
                    };
                    *original.borrow_mut() = completed.borrow().clone();
                    *original.meta.borrow_mut() = std::mem::take(&mut *completed.meta.borrow_mut());
                    *module.namespace.borrow_mut() = cached.clone();
                }
            }
            for id in &created {
                self.link_imports(id)?;
            }
            Ok(true)
        })();
        if result.is_err() {
            for id in &created {
                self.module_graph.borrow_mut().remove(id);
                self.modules.borrow_mut().remove(id);
            }
        }
        self.republish_roots();
        result
    }
    fn discover_module(
        &mut self,
        name: &str,
        created: &mut Vec<String>,
        depth: usize,
    ) -> Result<bool, VmErr> {
        if depth >= MAX_CALL_DEPTH {
            return Err(crate::value::limit_err(
                "Maximum module graph depth exceeded",
            ));
        }
        if self.module_graph.borrow().records.contains_key(name) {
            return Ok(true);
        }
        if !self.module_sources.borrow().contains_key(name) {
            if self.modules.borrow().contains_key(name) {
                return Ok(true);
            }
            let Some(loader) = self.module_loader.clone() else {
                return Ok(false);
            };
            let loaded = loader.load(name)?;
            if loaded.id != name {
                return Err(VmErr::Msg(
                    "module loader changed canonical identity".into(),
                ));
            }
            if name.starts_with("file:") {
                self.define_module_file_url(name, name.into());
            }
            self.define_module(name, loaded.source);
        }
        let source = self
            .module_sources
            .borrow()
            .get(name)
            .cloned()
            .expect("loaded source");
        let mut program = self.prepare_source(&source, SourceKind::Module)?;
        program.executable = match crate::bytecode::compiler::compile_linked_program(&linked_body(
            &program.statements,
        )) {
            Ok(code) => {
                program.fallback_reason = None;
                crate::bytecode::verify_module(&code)
                    .map_err(|error| VmErr::Msg(format!("internal error: {error}")))?;
                Executable::Bytecode(code)
            }
            Err(unsupported) => {
                program.fallback_reason = Some(unsupported.reason);
                Executable::Ast
            }
        };
        let scope = self
            .modules
            .borrow()
            .get(name)
            .and_then(|m| m.scope.clone())
            .unwrap_or_else(|| self.new_module_scope());
        self.modules.borrow_mut().insert(
            name.into(),
            Module {
                namespace: Rc::new(RefCell::new(None)),
                exports: HashMap::new(),
                default: None,
                scope: Some(scope.clone()),
            },
        );
        scope.borrow_mut().set_module_context(name);
        created.push(name.into());
        self.module_graph.borrow_mut().records.insert(
            name.into(),
            LinkedModule {
                program: program.clone(),
                requests: Vec::new(),
                state: ModuleState::Linked,
                evaluation: None,
            },
        );
        let outer = self.cur_mod.replace(name.into());
        let outer_scope = std::mem::replace(&mut self.global, scope);
        let declarations = flat(&program.statements);
        let result = (|| {
            self.hoist_vars(&program.statements)?;
            self.hoist_lexical(&program.statements)?;
            let mut explicit = HashSet::new();
            for stmt in &declarations {
                match stmt {
                    Statement::ExportNamed {
                        specifiers,
                        source: None,
                        ..
                    } => {
                        for (local, export) in specifiers {
                            if !explicit.insert(export.clone()) {
                                return Err(syntax(format!("Duplicate export '{export}'")));
                            }
                            let imported = declarations.iter().any(|stmt| matches!(stmt, Statement::Import { default, named, namespace, .. } if default.as_ref() == Some(local) || namespace.as_ref() == Some(local) || named.iter().any(|(_, n)| n == local)));
                            let cell = self.global.borrow_mut().export_cell(local);
                            let Some(cell) = cell else {
                                if imported {
                                    continue;
                                }
                                return Err(syntax(format!("Export '{local}' is not defined")));
                            };
                            if export == "default" {
                                self.current_module().default = Some(Value::Binding(cell));
                            } else {
                                self.current_module()
                                    .exports
                                    .insert(export.clone(), Value::Binding(cell));
                            }
                        }
                    }
                    Statement::ExportNamed { specifiers, .. } => {
                        for (_, export) in specifiers {
                            if !explicit.insert(export.clone()) {
                                return Err(syntax(format!("Duplicate export '{export}'")));
                            }
                        }
                    }
                    Statement::ExportDefault(expr) => {
                        if !explicit.insert("default".into()) {
                            return Err(syntax("Duplicate default export"));
                        }
                        if let Some(local) = default_local(&program.statements, stmt) {
                            let cell = self
                                .global
                                .borrow_mut()
                                .export_cell(&local)
                                .expect("hoisted default declaration");
                            self.current_module().default = Some(Value::Binding(cell));
                            continue;
                        }
                        let initial = if matches!(expr.as_ref(), crate::parser::Expr::FnExpr { .. })
                        {
                            self.eval_expr(expr)?
                        } else {
                            Value::Uninitialized
                        };
                        self.current_module().default = Some(Value::Binding(crate::heap::tracked(
                            Rc::new(RefCell::new(initial)),
                        )));
                    }
                    Statement::ExportAll {
                        alias: Some(alias), ..
                    } if !explicit.insert(alias.clone()) => {
                        return Err(syntax(format!("Duplicate export '{alias}'")));
                    }
                    _ => {}
                }
            }
            // Imports may be re-exported locally; install placeholders before
            // resolving their source, then link_imports replaces those cells.
            let mut requests = Vec::new();
            for stmt in declarations {
                let request = match stmt {
                    Statement::Import { module, .. } => Some(module),
                    Statement::ExportNamed { source, .. } => source.as_ref(),
                    Statement::ExportAll { source, .. } => Some(source),
                    _ => None,
                };
                if let Some(request) = request {
                    let resolved = self
                        .resolve_module_request(request)?
                        .ok_or_else(|| syntax(format!("Module not found: {request}")))?;
                    if !requests.iter().any(|(old, _)| old == request) {
                        requests.push((request.clone(), resolved));
                    }
                }
            }
            self.module_graph
                .borrow_mut()
                .records
                .get_mut(name)
                .unwrap()
                .requests = requests.clone();
            for (_, dependency) in requests {
                if !self.discover_module(&dependency, created, depth + 1)? {
                    return Err(syntax(format!("Module not found: {dependency}")));
                }
            }
            Ok(())
        })();
        self.global = outer_scope;
        self.cur_mod = outer;
        result.map(|()| true)
    }
    pub(super) fn is_linked_module(&self) -> bool {
        self.cur_mod
            .as_ref()
            .is_some_and(|id| self.module_graph.borrow().records.contains_key(id))
    }
    fn request_target(&self, id: &str, request: &str) -> Option<String> {
        self.module_graph
            .borrow()
            .records
            .get(id)?
            .requests
            .iter()
            .find(|(r, _)| r == request)
            .map(|(_, id)| id.clone())
    }
    fn exported_names(&self, id: &str, seen: &mut HashSet<String>) -> HashSet<String> {
        if !seen.insert(id.into()) {
            return HashSet::new();
        }
        let program = self
            .module_graph
            .borrow()
            .records
            .get(id)
            .map(|r| r.program.clone());
        let mut names = HashSet::new();
        if let Some(program) = program {
            for stmt in flat(&program.statements) {
                match stmt {
                    Statement::ExportNamed { specifiers, .. } => {
                        names.extend(specifiers.iter().map(|(_, e)| e.clone()))
                    }
                    Statement::ExportDefault(_) => {
                        names.insert("default".into());
                    }
                    Statement::ExportAll {
                        source,
                        alias: Some(alias),
                        ..
                    } => {
                        let _ = source;
                        names.insert(alias.clone());
                    }
                    Statement::ExportAll {
                        source,
                        alias: None,
                        ..
                    } => {
                        if let Some(target) = self.request_target(id, source) {
                            names.extend(
                                self.exported_names(&target, seen)
                                    .into_iter()
                                    .filter(|n| n != "default"),
                            );
                        }
                    }
                    _ => {}
                }
            }
        } else if let Some(module) = self.module(id) {
            names.extend(module.exports.keys().cloned());
            if module.default.is_some() {
                names.insert("default".into());
            }
        }
        names
    }
    fn resolve_export(
        &self,
        id: &str,
        name: &str,
        seen: &mut HashSet<(String, String)>,
    ) -> Result<Option<Value>, VmErr> {
        if !seen.insert((id.into(), name.into())) {
            return Ok(None);
        }
        let program = self
            .module_graph
            .borrow()
            .records
            .get(id)
            .map(|r| r.program.clone());
        let Some(program) = program else {
            return Ok(self.module(id).and_then(|m| {
                if name == "default" {
                    m.default
                } else {
                    m.exports.get(name).cloned()
                }
            }));
        };
        for stmt in flat(&program.statements) {
            match stmt {
                Statement::ExportDefault(_) if name == "default" => {
                    return Ok(self.module(id).and_then(|m| m.default));
                }
                Statement::ExportNamed {
                    specifiers, source, ..
                } => {
                    if let Some((local, _)) =
                        specifiers.iter().find(|(_, exported)| exported == name)
                    {
                        if let Some(source) = source {
                            let target = self.request_target(id, source).expect("resolved request");
                            return self
                                .resolve_export(&target, local, seen)?
                                .map(Some)
                                .ok_or_else(|| {
                                    syntax(format!(
                                        "Module '{source}' has no unambiguous export '{local}'"
                                    ))
                                });
                        }
                        // A local export can name an import; follow its source
                        // rather than creating a second, disconnected cell.
                        for imported in flat(&program.statements) {
                            if let Statement::Import {
                                module,
                                default,
                                named,
                                namespace,
                                ..
                            } = imported
                            {
                                let target =
                                    self.request_target(id, module).expect("resolved import");
                                if default.as_deref() == Some(local) {
                                    return self.resolve_export(&target, "default", seen);
                                }
                                if let Some((remote, _)) = named.iter().find(|(_, n)| n == local) {
                                    return self.resolve_export(&target, remote, seen);
                                }
                                if namespace.as_deref() == Some(local) {
                                    return self
                                        .module(&target)
                                        .map(|m| Self::namespace_object(&m))
                                        .transpose();
                                }
                            }
                        }
                        return Ok(self.module(id).and_then(|m| {
                            if name == "default" {
                                m.default
                            } else {
                                m.exports.get(name).cloned()
                            }
                        }));
                    }
                }
                Statement::ExportAll {
                    source,
                    alias: Some(alias),
                    ..
                } if alias == name => {
                    let target = self.request_target(id, source).expect("resolved request");
                    return self
                        .module(&target)
                        .map(|m| Self::namespace_object(&m))
                        .transpose();
                }
                _ => {}
            }
        }
        if name == "default" {
            return Ok(None);
        }
        let mut resolved: Option<Value> = None;
        for stmt in flat(&program.statements) {
            if let Statement::ExportAll {
                source,
                alias: None,
                ..
            } = stmt
            {
                let target = self.request_target(id, source).expect("resolved star");
                if let Some(candidate) = self.resolve_export(&target, name, &mut seen.clone())? {
                    if let Some(previous) = &resolved {
                        let same = match (previous, &candidate) {
                            (Value::Binding(a), Value::Binding(b)) => Rc::ptr_eq(a, b),
                            _ => strict_equals(previous, &candidate),
                        };
                        if !same {
                            return Ok(None);
                        }
                    }
                    resolved = Some(candidate);
                }
            }
        }
        Ok(resolved)
    }
    fn link_imports(&mut self, id: &str) -> Result<(), VmErr> {
        let program = self.module_graph.borrow().records[id].program.clone();
        let scope = self.module_scope(id);
        let outer = std::mem::replace(&mut self.global, scope);
        let result = (|| {
            for stmt in flat(&program.statements) {
                if let Statement::Import {
                    module,
                    default,
                    named,
                    namespace,
                    ..
                } = stmt
                {
                    let mut locals = named
                        .iter()
                        .map(|(_, local)| local.as_str())
                        .collect::<Vec<_>>();
                    locals.extend(default.as_deref());
                    locals.extend(namespace.as_deref());
                    let mut unique = HashSet::new();
                    for local in locals {
                        if !unique.insert(local)
                            || self.global.borrow().own_binding(local).is_some()
                        {
                            return Err(syntax(format!("Duplicate module binding '{local}'")));
                        }
                    }
                    let target = self.request_target(id, module).expect("resolved import");
                    let mut bindings = named.clone();
                    if let Some(local) = default {
                        bindings.push(("default".into(), local.clone()));
                    }
                    for (remote, local) in bindings {
                        let value = self
                            .resolve_export(&target, &remote, &mut HashSet::new())?
                            .ok_or_else(|| {
                                syntax(format!(
                                    "Module '{module}' has no unambiguous export '{remote}'"
                                ))
                            })?;
                        self.bind_import(&local, value)?;
                    }
                    if let Some(local) = namespace {
                        let value =
                            Self::namespace_object(&self.module(&target).expect("linked module"))?;
                        self.declare_binding(local, value, BindKind::Const, true)?;
                    }
                }
            }
            Ok(())
        })();
        self.global = outer;
        result
    }
    pub(super) fn evaluate_linked_module(&mut self, id: &str) -> Result<(), VmErr> {
        let Some((state, program, requests)) = self
            .module_graph
            .borrow()
            .records
            .get(id)
            .map(|r| (r.state.clone(), r.program.clone(), r.requests.clone()))
        else {
            return Ok(());
        };
        match state {
            ModuleState::Evaluated => return Ok(()),
            ModuleState::Evaluating => {
                let in_body = self
                    .cur_mod
                    .as_ref()
                    .is_some_and(|id| self.evaluating.borrow().contains(id));
                if !in_body {
                    let pending = self
                        .module_graph
                        .borrow()
                        .records
                        .get(id)
                        .and_then(|record| record.evaluation.clone());
                    if let Some(promise) = pending {
                        self.perform_await(Value::Promise(promise))?;
                    }
                }
                return Ok(());
            }
            ModuleState::Failed(value) => return Err(VmErr::Throw(value)),
            ModuleState::Linked => {}
        }
        self.module_graph
            .borrow_mut()
            .records
            .get_mut(id)
            .unwrap()
            .state = ModuleState::Evaluating;
        self.evaluating.borrow_mut().insert(id.into());
        let outer = self.cur_mod.replace(id.into());
        let scope = self.module_scope(id);
        let outer_scope = std::mem::replace(&mut self.global, scope);
        let result = (|| {
            for (_, dependency) in requests {
                self.evaluate_linked_module(&dependency)?;
            }
            match &program.executable {
                Executable::Bytecode(code) => self.run_bytecode_module(code).map(|_| ()),
                Executable::Ast => self.run(&linked_body(&program.statements)).map(|_| ()),
            }
        })();
        self.global = outer_scope;
        self.cur_mod = outer;
        self.evaluating.borrow_mut().remove(id);
        let mut graph = self.module_graph.borrow_mut();
        let record = graph.records.get_mut(id).unwrap();
        match &result {
            Ok(()) => record.state = ModuleState::Evaluated,
            Err(error) => {
                let value = error_value(error);
                record.state = ModuleState::Failed(value);
            }
        }
        result
    }
}
