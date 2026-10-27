//! Annex B block-function eligibility and statement-time variable updates.
use super::*;
use crate::parser::{ForBinding, ForInit};
use std::collections::HashSet;

fn functions(body: &[Statement]) -> Vec<String> {
    body.iter()
        .filter_map(|s| match labeled_function(s) {
            Some(Statement::FnDecl { name, .. }) => Some(name.clone()),
            _ => None,
        })
        .collect()
}
fn collect<'a>(
    body: &'a [Statement],
    outer: &HashSet<String>,
    block: bool,
    out: &mut Vec<(&'a Statement, String)>,
) {
    let mut blocked = outer.clone();
    blocked.extend(crate::parser::top_lexical_names(body));
    let mut nested = blocked.clone();
    if block {
        nested.extend(functions(body));
    }
    for statement in body {
        match statement {
            Statement::FnDecl {
                name,
                is_async: false,
                is_generator: false,
                annex_b_statement,
                ..
            } if (block || *annex_b_statement) && !blocked.contains(name) => {
                out.push((statement, name.clone()))
            }
            Statement::Block(b)
            | Statement::With { body: b, .. }
            | Statement::While { body: b, .. }
            | Statement::DoWhile { body: b, .. }
            | Statement::ResourceForOf { body: b, .. } => collect(b, &nested, true, out),
            Statement::Declarations(b)
            | Statement::ResourceDeclaration {
                declarations: b, ..
            } => collect(b, &blocked, block, out),
            Statement::If { then, else_, .. } => {
                collect(then, &nested, true, out);
                if let Some(b) = else_ {
                    collect(b, &nested, true, out);
                }
            }
            Statement::For { init, body, .. } => {
                let mut names = nested.clone();
                if let Some(init) = init {
                    match init.as_ref() {
                        ForInit::Var { kind, decls } if *kind != VarKind::Var => {
                            names.extend(decls.iter().map(|(n, _)| n.clone()))
                        }
                        ForInit::Pattern {
                            kind,
                            pattern,
                            trailing,
                            ..
                        } if *kind != VarKind::Var => {
                            names.extend(pattern_names(pattern));
                            names.extend(trailing.iter().map(|(n, _)| n.clone()));
                        }
                        _ => {}
                    }
                }
                collect(body, &names, true, out);
            }
            Statement::ForIn { binding, body, .. } | Statement::ForOf { binding, body, .. } => {
                let mut names = nested.clone();
                if let ForBinding::Declaration { kind, pattern, .. } = binding
                    && *kind != VarKind::Var
                {
                    names.extend(pattern_names(pattern));
                }
                collect(body, &names, true, out);
            }
            Statement::Try {
                body,
                catch,
                finally,
            } => {
                collect(body, &nested, true, out);
                if let Some((parameter, b)) = catch {
                    let mut names = nested.clone();
                    if let Some(p) = parameter
                        && !matches!(p, crate::parser::Pattern::Ident(_))
                    {
                        names.extend(pattern_names(p));
                    }
                    collect(b, &names, true, out);
                }
                if let Some(b) = finally {
                    collect(b, &nested, true, out);
                }
            }
            Statement::Switch { cases, .. } => {
                let mut names = nested.clone();
                for case in cases {
                    names.extend(crate::parser::top_lexical_names(&case.body));
                }
                for case in cases {
                    collect(&case.body, &names, true, out);
                }
            }
            Statement::Labeled { body, .. } => {
                let names = if labeled_function(statement).is_some() {
                    &blocked
                } else {
                    &nested
                };
                collect(std::slice::from_ref(body.as_ref()), names, block, out)
            }
            _ => {}
        }
    }
}
impl Interpreter {
    pub(super) fn hoist_annex_functions(&mut self, body: &[Statement]) -> Result<(), VmErr> {
        if self.global.borrow().strict() {
            return Ok(());
        }
        let scope = self.global.clone();
        let variable = Environment::variable_environment(&scope);
        let mut blocked = HashSet::new();
        if !scope.borrow().is_global_scope() && !scope.borrow().is_eval_scope() {
            // Before var instantiation the function environment contains its
            // parameter bindings and (when needed) the arguments object.
            let frame = scope.borrow();
            blocked.extend(frame.own_keys());
            if frame.implicit_arguments {
                blocked.remove("arguments");
            }
        }
        let mut candidates = Vec::new();
        collect(body, &blocked, false, &mut candidates);
        for (statement, name) in candidates {
            // Eval must also respect intervening lexical environments.
            let mut current = scope.clone();
            let mut conflict = false;
            loop {
                if current.borrow().lexical_declaration(&name) {
                    conflict = true;
                    break;
                }
                if Rc::ptr_eq(&current, &variable) {
                    break;
                }
                let next = current.borrow().parent_env();
                let Some(next) = next else { break };
                current = next;
            }
            if conflict {
                continue;
            }
            // Non-declarable global names suppress the web extension.
            if variable.borrow().is_global_scope() {
                if let Some((_, attrs)) = variable.borrow().global_property(&name) {
                    if !attrs.configurable && (!attrs.writable || !attrs.enumerable) {
                        continue;
                    }
                } else if let Some(object) = variable.borrow().global_object()
                    && !self.is_extensible(&object)?
                {
                    continue;
                }
            }
            self.hoist_var_binding_in(&scope, &name)?;
            scope
                .borrow_mut()
                .annex_functions
                .insert(statement as *const Statement as usize);
        }
        Ok(())
    }
    pub(super) fn evaluate_function_declaration(
        &mut self,
        statement: &Statement,
    ) -> Result<Value, VmErr> {
        let key = statement as *const Statement as usize;
        let mut scope = self.global.clone();
        loop {
            if scope.borrow().annex_functions.contains(&key) {
                let Statement::FnDecl { name, .. } = statement else {
                    unreachable!()
                };
                let value = self.global.borrow().get(name).unwrap_or(Value::Undefined);
                let target = Environment::variable_environment(&scope);
                self.set_binding_in(&target, name, value)?;
                break;
            }
            let next = scope.borrow().parent_env();
            let Some(next) = next else { break };
            scope = next;
        }
        Ok(Value::Undefined)
    }
}

pub(crate) fn eligible<'a>(
    body: &'a [Statement],
    parameters: &[String],
    _arguments: bool,
) -> Vec<(&'a Statement, String)> {
    let blocked: HashSet<String> = parameters
        .iter()
        .map(|name| name.trim_start_matches("...").to_owned())
        .collect();
    let mut result = Vec::new();
    collect(body, &blocked, false, &mut result);
    result
}
impl Interpreter {
    pub(crate) fn init_compiled_annex(&mut self, scope: &Env, name: &str) -> Result<(), VmErr> {
        if scope.borrow().strict() {
            return Ok(());
        }
        let variable = Environment::variable_environment(scope);
        if variable.borrow().lexical_declaration(name) {
            return Ok(());
        }
        if variable.borrow().is_global_scope() {
            if let Some((_, attrs)) = variable.borrow().global_property(name) {
                if !attrs.configurable && (!attrs.writable || !attrs.enumerable) {
                    return Ok(());
                }
            } else if let Some(object) = variable.borrow().global_object()
                && !self.is_extensible(&object)?
            {
                return Ok(());
            }
        }
        self.hoist_var_binding_in(scope, name)?;
        variable.borrow_mut().annex_names.insert(name.into());
        Ok(())
    }
    pub(crate) fn update_compiled_annex(
        &mut self,
        scope: &Env,
        name: &str,
        value: Value,
    ) -> Result<bool, VmErr> {
        if scope.borrow().strict() {
            return Ok(false);
        }
        let variable = Environment::variable_environment(scope);
        if !variable.borrow().annex_names.contains(name) {
            return Ok(false);
        }
        self.set_binding_in(&variable, name, value)?;
        Ok(true)
    }
}
