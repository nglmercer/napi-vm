//! Static semantics shared by cached parsing, eval and compilation.
use super::*;
use std::collections::HashSet;

#[derive(Clone, Default)]
struct Context {
    strict: bool,
    module: bool,
    import_meta: bool,
    top_level: bool,
    function: bool,
    new_target: bool,
    loops: usize,
    switches: usize,
    labels: Vec<(String, bool)>,
}

type Check = Result<(), String>;

pub(crate) fn use_strict(body: &[Statement]) -> bool {
    body.iter()
        .take_while(|s| matches!(s, Statement::Expr(Expr::String(_) | Expr::EscapedString(_))))
        .any(|s| matches!(s, Statement::Expr(Expr::String(text)) if text == "use strict"))
}

pub(super) fn validate(
    body: &[Statement],
    new_target: bool,
    strict: bool,
    goal: ParseGoal,
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
    statements(
        body,
        &Context {
            strict: strict || module || use_strict(body),
            module,
            import_meta: goal != ParseGoal::Script,
            top_level: true,
            new_target,
            ..Context::default()
        },
    )
}

fn binding(name: &str, ctx: &Context) -> Check {
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
    arrow: bool,
    unique: bool,
) -> Check {
    let ctx = Context {
        strict: outer.strict || use_strict(body),
        function: true,
        module: outer.module,
        import_meta: outer.import_meta,
        new_target: !arrow || outer.new_target,
        ..Context::default()
    };
    parameters(
        params,
        &ctx,
        unique || arrow || params.iter().any(|p| p.starts_with("...")),
    )?;
    // Parameters cannot collide with direct lexical declarations in the body.
    let lexical = lexical_names(body)?;
    if params
        .iter()
        .any(|p| lexical.contains(p.trim_start_matches("...")))
    {
        return Err("parameter conflicts with lexical declaration".into());
    }
    statements(body, &ctx)
}

fn lexical_names(body: &[Statement]) -> Result<HashSet<String>, String> {
    let mut names = HashSet::new();
    for stmt in body {
        match stmt {
            Statement::Declarations(decls) => {
                for name in lexical_names(decls)? {
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
            Statement::ClassDecl { name, .. } if !names.insert(name.clone()) => {
                return Err(format!("duplicate lexical binding: {name}"));
            }
            _ => {}
        }
    }
    Ok(names)
}

fn statements(body: &[Statement], ctx: &Context) -> Check {
    let lexical = lexical_names(body)?;
    let mut vars = Vec::new();
    collect_var_declaration_names(body, &mut vars);
    for statement in body {
        if let Statement::FnDecl { name, .. } = statement {
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
            ..
        } => {
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
        } => {
            let mut own = ctx.clone();
            own.strict |= use_strict(body);
            binding(name, &own)?;
            function(params, body, ctx, false, *is_async || *is_generator)
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
            optional(superclass.as_deref(), ctx)?;
            class(body, ctx)
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
                    ForInit::Var { decls, .. } => {
                        for (name, expr) in decls {
                            binding(name, ctx)?;
                            optional(expr.as_ref(), ctx)?;
                        }
                    }
                    ForInit::Pattern {
                        pattern,
                        init,
                        trailing,
                        ..
                    } => {
                        pattern_check(pattern, ctx)?;
                        expression(init, ctx)?;
                        for (name, expr) in trailing {
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
        Statement::ForIn { name, obj, body } => {
            binding(name, ctx)?;
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
            name,
            pattern,
            iter,
            body,
            ..
        } => {
            if let Some(pattern) = pattern {
                pattern_check(pattern, ctx)?;
            } else {
                binding(name, ctx)?;
            }
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
        Statement::Declarations(body) => {
            for stmt in body {
                statement(stmt, ctx)?;
            }
            Ok(())
        }
        Statement::Labeled { label, body } => {
            if ctx.labels.iter().any(|(name, _)| name == label) {
                return Err(format!("duplicate label: {label}"));
            }
            let mut next = ctx.clone();
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
            if let Some((name, body)) = catch {
                binding(name, ctx)?;
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
                ..ctx.clone()
            };
            if cases.iter().filter(|case| case.test.is_none()).count() > 1 {
                return Err("duplicate default clause".into());
            }
            let body: Vec<_> = cases
                .iter()
                .flat_map(|case| case.body.iter().cloned())
                .collect();
            lexical_names(&body)?;
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
        Pattern::Rest(inner) => pattern_check(inner, ctx),
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
            for (key, value) in props {
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
        Pattern::Member { object, property } => {
            expression(object, ctx)?;
            expression(property, ctx)
        }
    }
}

fn simple_assignment_target(target: &Expr, ctx: &Context) -> Check {
    match target {
        Expr::Identifier(name) => binding(name, ctx),
        Expr::Member { .. } => expression(target, ctx),
        _ => Err("invalid assignment target".into()),
    }
}

fn assignment_target(target: &Expr, ctx: &Context) -> Check {
    match target {
        Expr::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                match item {
                    Expr::Undefined => {}
                    Expr::Spread(inner) if index + 1 == items.len() => {
                        assignment_target(inner, ctx)?
                    }
                    Expr::Spread(_) => return Err("rest element must be last".into()),
                    _ => assignment_target(item, ctx)?,
                }
            }
            Ok(())
        }
        Expr::Object(props) => {
            for (index, prop) in props.iter().enumerate() {
                match prop {
                    ObjectProp::Shorthand(name) => binding(name, ctx)?,
                    ObjectProp::KeyValue(_, target) => assignment_target(target, ctx)?,
                    ObjectProp::Computed(key, target) => {
                        expression(key, ctx)?;
                        assignment_target(target, ctx)?;
                    }
                    ObjectProp::Spread(target) if index + 1 == props.len() => {
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

fn expression(expr: &Expr, ctx: &Context) -> Check {
    match expr {
        Expr::ImportMeta if !ctx.import_meta => Err("import.meta outside a module".into()),
        Expr::NewTarget if !ctx.new_target => Err("new.target outside a function".into()),
        Expr::Array(items) | Expr::Template { exprs: items, .. } => {
            for item in items {
                expression(item, ctx)?;
            }
            Ok(())
        }
        Expr::Object(props) => {
            for prop in props {
                match prop {
                    ObjectProp::Shorthand(_) => {}
                    ObjectProp::KeyValue(_, value) | ObjectProp::Spread(value) => {
                        expression(value, ctx)?
                    }
                    ObjectProp::Computed(key, value) => {
                        expression(key, ctx)?;
                        expression(value, ctx)?;
                    }
                    ObjectProp::Method { params, body, .. } => {
                        function(params, body, ctx, false, true)?
                    }
                    ObjectProp::Getter { body, .. } => function(&[], body, ctx, false, true)?,
                    ObjectProp::Setter { param, body, .. } => {
                        function(std::slice::from_ref(param), body, ctx, false, true)?
                    }
                }
            }
            Ok(())
        }
        Expr::Binary { left, right, .. } => {
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
            expression(object, ctx)?;
            if *computed {
                expression(property, ctx)?;
            }
            Ok(())
        }
        Expr::Unary { op, operand, .. } => {
            if ctx.strict && *op == UnOp::Delete && matches!(operand.as_ref(), Expr::Identifier(_))
            {
                return Err("delete of an unqualified identifier in strict mode".into());
            }
            if matches!(op, UnOp::Inc | UnOp::Dec) {
                simple_assignment_target(operand, ctx)
            } else {
                expression(operand, ctx)
            }
        }
        Expr::Assignment { target, value, op } => {
            if *op == AssignOp::Assign {
                assignment_target(target, ctx)?;
            } else {
                simple_assignment_target(target, ctx)?;
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
            expression(callee, ctx)?;
            for arg in args {
                expression(arg, ctx)?;
            }
            Ok(())
        }
        Expr::TaggedTemplate { tag, exprs, .. } => {
            expression(tag, ctx)?;
            for expr in exprs {
                expression(expr, ctx)?;
            }
            Ok(())
        }
        Expr::ArrowFn { params, body, .. } => match body.as_ref() {
            ExprOrBlock::Block(body) => function(params, body, ctx, true, true),
            ExprOrBlock::Expr(expr) => {
                parameters(params, ctx, true)?;
                expression(
                    expr,
                    &Context {
                        function: true,
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
            if let Some(name) = name {
                binding(name, &own)?;
            }
            function(params, body, ctx, false, *is_async || *is_generator)
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
            optional(superclass.as_deref(), ctx)?;
            class(body, ctx)
        }
        Expr::Spread(value)
        | Expr::Await(value)
        | Expr::YieldFrom(value)
        | Expr::DynamicImport(value) => expression(value, ctx),
        Expr::Yield(value) => optional(value.as_deref(), ctx),
        Expr::Number(_)
        | Expr::BigIntLiteral(_)
        | Expr::String(_)
        | Expr::EscapedString(_)
        | Expr::Regex(_, _)
        | Expr::Bool(_)
        | Expr::Null
        | Expr::Undefined
        | Expr::Identifier(_)
        | Expr::This
        | Expr::Super
        | Expr::ImportMeta
        | Expr::NewTarget => Ok(()),
    }
}

fn class(body: &[ClassMember], outer: &Context) -> Check {
    let ctx = Context {
        strict: true,
        ..outer.clone()
    };
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
            ClassMember::Method { params, body, .. } => function(params, body, &ctx, false, true)?,
            ClassMember::Getter { body, .. } => function(&[], body, &ctx, false, true)?,
            ClassMember::Setter { param, body, .. } => {
                function(std::slice::from_ref(param), body, &ctx, false, true)?
            }
            ClassMember::Field { init, .. } => optional(
                init.as_ref(),
                &Context {
                    new_target: true,
                    ..ctx.clone()
                },
            )?,
            ClassMember::StaticBlock { body } => statements(
                body,
                &Context {
                    new_target: true,
                    function: false,
                    loops: 0,
                    switches: 0,
                    labels: Vec::new(),
                    ..ctx.clone()
                },
            )?,
        }
    }
    Ok(())
}
