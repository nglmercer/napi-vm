/// Binary operators, resolved at parse time so evaluation matches an integer
/// discriminant instead of a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    Eq,
    Neq,
    Seq,
    Sneq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    Nullish,
    Comma,
    Instanceof,
    In,
}

/// Unary operators (including prefix/postfix increment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
    Pos,
    BitNot,
    Typeof,
    Void,
    Delete,
    Inc,
    Dec,
}

/// Assignment operators. `Assign` is plain `=`; the rest are compound and map
/// to a binary operation via `bin_op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
}

/// Which condition makes a logical assignment write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalAssignOp {
    /// `&&=`: assign when the current value is truthy.
    And,
    /// `||=`: assign when the current value is falsy.
    Or,
    /// `??=`: assign when the current value is `null` or `undefined`.
    Nullish,
}

impl AssignOp {
    /// The binary operation behind a compound assignment (`+=` → `Add`).
    /// `None` for plain `=`.
    pub fn bin_op(self) -> Option<BinOp> {
        Some(match self {
            AssignOp::Assign => return None,
            AssignOp::Add => BinOp::Add,
            AssignOp::Sub => BinOp::Sub,
            AssignOp::Mul => BinOp::Mul,
            AssignOp::Div => BinOp::Div,
            AssignOp::Mod => BinOp::Mod,
            AssignOp::Pow => BinOp::Pow,
            AssignOp::BitAnd => BinOp::BitAnd,
            AssignOp::BitOr => BinOp::BitOr,
            AssignOp::BitXor => BinOp::BitXor,
            AssignOp::Shl => BinOp::Shl,
            AssignOp::Shr => BinOp::Shr,
            AssignOp::UShr => BinOp::UShr,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportPhase {
    Evaluation,
    Source,
    Deferred,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A legacy literal spelling permitted only in non-strict code.
    LegacyLiteral(Box<Expr>),
    /// Parentheses preserve references but restrict cover-pattern syntax.
    Parenthesized(Box<Expr>),
    Number(f64),
    /// A `BigInt` literal, carrying its digits.
    BigIntLiteral(String),
    String(crate::JsString),
    /// Decoded string with escaped spelling, retained for directive semantics.
    EscapedString(crate::JsString),
    /// `/pattern/flags`.
    Regex(crate::JsString, String),
    Bool(bool),
    Null,
    Undefined,
    Identifier(String),
    Array {
        items: Vec<Expr>,
        trailing_comma: bool,
    },
    Object {
        props: Vec<ObjectProp>,
        trailing_comma: bool,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnOp,
        operand: Box<Expr>,
        prefix: bool,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Member {
        object: Box<Expr>,
        property: Box<Expr>,
        computed: bool,
    },
    /// `` tag`a${x}b` ``: the tag is called with the array of literal chunks
    /// (carrying a `raw` companion array) followed by the interpolated values.
    TaggedTemplate {
        tag: Box<Expr>,
        cooked: Vec<Option<crate::JsString>>,
        raw: Vec<crate::JsString>,
        exprs: Vec<Expr>,
    },
    Assignment {
        target: Box<Expr>,
        op: AssignOp,
        value: Box<Expr>,
    },
    /// `a &&= b`, `a ||= b`, `a ??= b`.
    ///
    /// Separate from [`Expr::Assignment`] because these short-circuit: `b` is
    /// evaluated, and the write performed, only when the current value calls
    /// for it. A compound assignment always does both.
    LogicalAssignment {
        target: Box<Expr>,
        op: LogicalAssignOp,
        value: Box<Expr>,
    },
    Conditional {
        test: Box<Expr>,
        consequent: Box<Expr>,
        alternate: Box<Expr>,
    },
    ArrowFn {
        params: Vec<String>,
        body: Box<ExprOrBlock>,
        is_async: bool,
    },
    /// `class { … }` / `class Named extends Base { … }` in expression
    /// position. The name, when present, binds only inside the class body.
    ClassExpr {
        name: Option<String>,
        superclass: Option<Box<Expr>>,
        body: Vec<ClassMember>,
    },
    FnExpr {
        name: Option<String>,
        params: Vec<String>,
        body: Vec<Statement>,
        is_async: bool,
        is_generator: bool,
    },
    New {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Spread(Box<Expr>),
    This,
    Super,
    ImportMeta,
    NewTarget,
    /// `import(specifier)`: resolves to the module's namespace object.
    DynamicImport {
        specifier: Box<Expr>,
        options: Option<Box<Expr>>,
        phase: ImportPhase,
    },
    Template {
        quasis: Vec<crate::JsString>,
        exprs: Vec<Expr>,
    },
    OptionalChain {
        object: Box<Expr>,
        property: Box<Expr>,
        computed: bool,
    },
    Await(Box<Expr>),
    Yield(Option<Box<Expr>>),
    /// `yield* iterable` -- delegate to another iterator, yielding each of its
    /// values in turn and evaluating to that iterator's return value.
    YieldFrom(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectProp {
    CoverInitializedName {
        name: String,
        initializer: Expr,
    },
    Shorthand(String),
    KeyValue(String, Expr),
    Computed(Expr, Expr),
    ComputedMethod {
        key: Expr,
        params: Vec<String>,
        body: Vec<Statement>,
        is_async: bool,
        is_generator: bool,
        is_getter: bool,
        is_setter: bool,
    },
    Method {
        name: String,
        params: Vec<String>,
        body: Vec<Statement>,
        is_async: bool,
        is_generator: bool,
    },
    Getter {
        name: String,
        body: Vec<Statement>,
    },
    Setter {
        name: String,
        param: String,
        body: Vec<Statement>,
    },
    Spread(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprOrBlock {
    Expr(Box<Expr>),
    Block(Vec<Statement>),
}

impl Expr {
    pub(crate) fn unparenthesized(&self) -> &Self {
        match self {
            Self::Parenthesized(inner) => inner.unparenthesized(),
            _ => self,
        }
    }

    pub(crate) fn is_string_literal(&self) -> bool {
        match self {
            Self::String(_) | Self::EscapedString(_) => true,
            Self::LegacyLiteral(inner) => inner.is_string_literal(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    ResourceForOf {
        name: String,
        iter: Box<Expr>,
        body: Vec<Statement>,
        is_await: bool,
        await_disposal: bool,
    },
    ResourceDeclaration {
        declarations: Vec<Statement>,
        is_await: bool,
    },
    Expr(Expr),
    VarDecl {
        kind: VarKind,
        name: String,
        init: Option<Box<Expr>>,
        destructuring: Option<Box<Pattern>>,
    },
    FnDecl {
        name: String,
        params: Vec<String>,
        body: Vec<Statement>,
        is_async: bool,
        is_generator: bool,

        /// Legacy declaration in an if clause or labelled statement.
        annex_b_statement: bool,
    },
    ClassDecl {
        name: String,
        superclass: Option<Box<Expr>>,
        body: Vec<ClassMember>,
    },
    Return(Option<Box<Expr>>),
    If {
        test: Box<Expr>,
        then: Vec<Statement>,
        else_: Option<Vec<Statement>>,
    },
    With {
        object: Box<Expr>,
        body: Vec<Statement>,
    },
    While {
        test: Box<Expr>,
        body: Vec<Statement>,
    },
    DoWhile {
        test: Box<Expr>,
        body: Vec<Statement>,
    },
    For {
        init: Option<Box<ForInit>>,
        test: Option<Box<Expr>>,
        update: Option<Box<Expr>>,
        body: Vec<Statement>,
    },
    ForIn {
        binding: ForBinding,
        obj: Box<Expr>,
        body: Vec<Statement>,
    },
    ForOf {
        binding: ForBinding,
        iter: Box<Expr>,
        body: Vec<Statement>,
        is_await: bool,
    },
    Block(Vec<Statement>),
    /// Several declarators from one `let`/`const`/`var` statement:
    /// `let a = 1, b = 2;`.
    ///
    /// Transparent to scoping -- unlike [`Statement::Block`], it introduces no
    /// environment, so the names land in the enclosing scope. It exists
    /// because one statement can only return one `Statement`.
    Declarations(Vec<Statement>),
    /// Internal constructor entry, before parameter and body initialization.
    ClassInitialization {
        derived: bool,
        /// Implicit derived constructors forward the argument list without iteration.
        forward_rest: Option<String>,
        fields: Vec<Statement>,
    },
    /// Non-simple formal parameters run before body declaration instantiation.
    ParameterInitialization {
        params: Vec<String>,
        initializers: Vec<Statement>,
        /// Base-class fields run before constructor parameter initialization.
        fields: Vec<Statement>,
    },
    Labeled {
        label: String,
        body: Box<Statement>,
    },
    Break,
    Continue,
    LabeledBreak(String),
    LabeledContinue(String),
    Throw(Box<Expr>),
    Try {
        body: Vec<Statement>,
        catch: Option<(Option<Pattern>, Vec<Statement>)>,
        finally: Option<Vec<Statement>>,
    },
    Switch {
        disc: Box<Expr>,
        cases: Vec<SwitchCase>,
    },
    ExportDefault(Box<Expr>),
    ExportNamed {
        attributes: Vec<(String, crate::JsString)>,
        specifiers: Vec<(String, String)>,
        source: Option<String>,
    },
    /// `export * from 'm'` and `export * as ns from 'm'`. With `alias`, the
    /// other module's namespace object is exported under that one name;
    /// without it, every named export of `m` is re-exported.
    ExportAll {
        attributes: Vec<(String, crate::JsString)>,
        source: String,
        alias: Option<String>,
    },
    Import {
        attributes: Vec<(String, crate::JsString)>,
        module: String,
        default: Option<String>,
        named: Vec<(String, String)>,
        namespace: Option<String>,
    },
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VarKind {
    Var,
    Let,
    Const,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ForBinding {
    Declaration {
        kind: VarKind,
        pattern: Pattern,
        initializer: Option<Box<Expr>>,
    },
    Assignment(Box<Expr>),
}

impl ForBinding {
    pub fn declared_names(&self) -> Vec<String> {
        match self {
            Self::Declaration { pattern, .. } => pattern_names(pattern),
            Self::Assignment(_) => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ForInit {
    Var {
        kind: VarKind,
        decls: Vec<(String, Option<Expr>)>,
    },
    /// `for (let {a} = x, i = 0; …)`: a pattern head with optional
    /// trailing identifier declarators. (A second pattern in the same head
    /// stays a syntax error.)
    Pattern {
        kind: VarKind,
        pattern: Pattern,
        init: Expr,
        trailing: Vec<(String, Option<Expr>)>,
    },
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    pub test: Option<Expr>,
    pub body: Vec<Statement>,
}

/// A class member name: written out, or `[expr]` evaluated once when the
/// class is defined.
#[derive(Debug, Clone, PartialEq)]
pub enum MemberName {
    Static(String),
    Private(String),
    Computed(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClassMember {
    Method {
        name: MemberName,
        is_static: bool,
        params: Vec<String>,
        body: Vec<Statement>,
        is_async: bool,
        is_generator: bool,
    },
    Field {
        name: MemberName,
        is_static: bool,
        init: Option<Expr>,
    },
    /// `static { … }`: runs once against the class, after its static fields
    /// are installed, with `this` bound to the class.
    StaticBlock { body: Vec<Statement> },
    Getter {
        name: MemberName,
        is_static: bool,
        body: Vec<Statement>,
    },
    Setter {
        name: MemberName,
        param: String,
        is_static: bool,
        body: Vec<Statement>,
    },
}

/// Source declarations for eval's lexical validation, independent of which
/// private storage forms the runtime currently installs.
pub(crate) fn class_private_declarations(body: &[ClassMember]) -> Vec<String> {
    body.iter()
        .filter_map(|member| {
            let name = match member {
                ClassMember::Method { name, .. }
                | ClassMember::Field { name, .. }
                | ClassMember::Getter { name, .. }
                | ClassMember::Setter { name, .. } => name,
                ClassMember::StaticBlock { .. } => return None,
            };
            match name {
                MemberName::Private(name) => Some(name.clone()),
                _ => None,
            }
        })
        .collect()
}

/// An object-pattern key: a static name, or `[expr]` evaluated at bind time.
#[derive(Debug, Clone, PartialEq)]
pub enum PatternKey {
    Name(String),
    Computed(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// An array-pattern elision consumes a position without binding a name.
    Elision,
    Ident(String),
    Array(Vec<Pattern>),
    Object(Vec<(PatternKey, Option<Pattern>)>),
    Rest(Box<Pattern>),
    Default(Box<Pattern>, Box<Expr>),
    /// A property as a destructuring target: `[o.p] = [1]`. Only reachable
    /// from a destructuring *assignment*, since a declaration binds names.
    Member {
        object: Box<Expr>,
        property: Box<Expr>,
        private: bool,
    },
}

impl Pattern {
    pub fn is_rest(&self) -> bool {
        matches!(self, Pattern::Rest(_))
    }
}

/// Every identifier a binding pattern introduces, in source order.
///
/// Used for hoisting: `const { a, b: [c] } = obj;` declares `a` and `c`, and
/// all of them must exist (in their dead zone) before the block's first
/// statement runs.
pub fn pattern_names(pattern: &Pattern) -> Vec<String> {
    let mut names = Vec::new();
    collect_pattern_names(pattern, &mut names);
    names
}

fn collect_pattern_names(pattern: &Pattern, out: &mut Vec<String>) {
    match pattern {
        Pattern::Elision => {}
        Pattern::Ident(name) => out.push(name.clone()),
        // A property target binds no name.
        Pattern::Member { .. } => {}
        Pattern::Array(items) => {
            for item in items {
                collect_pattern_names(item, out);
            }
        }
        Pattern::Object(props) => {
            for (key, sub) in props {
                match sub {
                    Some(sub) => collect_pattern_names(sub, out),
                    // Shorthand `{ a }` binds the key itself. A computed key
                    // always carries a target (`{ [k] }` is a syntax error).
                    None => {
                        if let PatternKey::Name(name) = key {
                            out.push(name.clone());
                        }
                    }
                }
            }
        }
        Pattern::Rest(inner) => collect_pattern_names(inner, out),
        Pattern::Default(inner, _) => collect_pattern_names(inner, out),
    }
}

/// Collect the names every `var` in `stmts` introduces, for hoisting to the
/// enclosing function or program scope.
///
/// Recurses through every construct a `var` can hide inside -- blocks, loops,
/// `if`, `try`, `switch`, labels -- but deliberately *not* into nested
/// functions or classes, which begin their own variable scope. Function
/// declarations are collected too: they are `var`-scoped, and the interpreter
/// defines them eagerly during hoisting.
/// Lexical names introduced by this statement list, without descending into
/// lexical blocks or nested functions. Used by declaration instantiation and
/// by the compiler's representation of the same declarations.
pub(crate) fn top_lexical_names(body: &[Statement]) -> Vec<String> {
    let mut names = Vec::new();
    for statement in body {
        match statement {
            Statement::VarDecl {
                name,
                kind,
                destructuring,
                ..
            } if *kind != VarKind::Var => {
                if let Some(pattern) = destructuring {
                    names.extend(pattern_names(pattern));
                } else {
                    names.push(name.clone());
                }
            }
            Statement::ClassDecl { name, .. } => names.push(name.clone()),
            Statement::Declarations(inner)
            | Statement::ResourceDeclaration {
                declarations: inner,
                ..
            } => names.extend(top_lexical_names(inner)),
            _ => {}
        }
    }
    names
}

pub fn collect_var_names(stmts: &[Statement], out: &mut Vec<String>) {
    collect_scoped_var_names(stmts, out, true);
}

/// Var declaration names without Annex B block-function hoisting.
pub(crate) fn collect_var_declaration_names(stmts: &[Statement], out: &mut Vec<String>) {
    collect_scoped_var_names(stmts, out, false);
}

fn collect_scoped_var_names(stmts: &[Statement], out: &mut Vec<String>, functions: bool) {
    for stmt in stmts {
        collect_stmt_var_names(stmt, out, functions);
    }
}

fn collect_stmt_var_names(stmt: &Statement, out: &mut Vec<String>, functions: bool) {
    match stmt {
        Statement::VarDecl {
            kind: VarKind::Var,
            name,
            destructuring,
            ..
        } => match destructuring {
            Some(pattern) => collect_pattern_names(pattern, out),
            None => out.push(name.clone()),
        },
        // Other declaration kinds are lexical: block-scoped, handled elsewhere.
        Statement::VarDecl { .. } | Statement::ClassDecl { .. } => {}
        // A function declaration's *name* is var-scoped; its body is not.
        Statement::FnDecl { name, .. } => {
            if functions {
                out.push(name.clone());
            }
        }
        Statement::Block(body)
        | Statement::Declarations(body)
        | Statement::ResourceDeclaration {
            declarations: body, ..
        }
        | Statement::With { body, .. } => collect_scoped_var_names(body, out, functions),
        Statement::If { then, else_, .. } => {
            collect_scoped_var_names(then, out, functions);
            if let Some(else_) = else_ {
                collect_scoped_var_names(else_, out, functions);
            }
        }
        Statement::While { body, .. }
        | Statement::DoWhile { body, .. }
        | Statement::ResourceForOf { body, .. } => collect_scoped_var_names(body, out, functions),
        Statement::ForIn { binding, body, .. } | Statement::ForOf { binding, body, .. } => {
            if let ForBinding::Declaration {
                kind: VarKind::Var,
                pattern,
                ..
            } = binding
            {
                out.extend(pattern_names(pattern));
            }
            collect_scoped_var_names(body, out, functions);
        }
        Statement::For { init, body, .. } => {
            if let Some(init) = init {
                match &**init {
                    ForInit::Var {
                        kind: VarKind::Var,
                        decls,
                    } => {
                        out.extend(decls.iter().map(|(name, _)| name.clone()));
                    }
                    ForInit::Pattern {
                        kind: VarKind::Var,
                        pattern,
                        trailing,
                        ..
                    } => {
                        out.extend(pattern_names(pattern));
                        out.extend(trailing.iter().map(|(name, _)| name.clone()));
                    }
                    _ => {}
                }
            }
            collect_scoped_var_names(body, out, functions);
        }
        Statement::Labeled { body, .. } => collect_stmt_var_names(body, out, functions),
        Statement::Try {
            body,
            catch,
            finally,
        } => {
            collect_scoped_var_names(body, out, functions);
            if let Some((_, catch_body)) = catch {
                collect_scoped_var_names(catch_body, out, functions);
            }
            if let Some(finally) = finally {
                collect_scoped_var_names(finally, out, functions);
            }
        }
        Statement::Switch { cases, .. } => {
            for case in cases {
                collect_scoped_var_names(&case.body, out, functions);
            }
        }
        Statement::Expr(_)
        | Statement::ClassInitialization { .. }
        | Statement::ParameterInitialization { .. }
        | Statement::Return(_)
        | Statement::Break
        | Statement::Continue
        | Statement::LabeledBreak(_)
        | Statement::LabeledContinue(_)
        | Statement::Throw(_)
        | Statement::ExportDefault(_)
        | Statement::ExportNamed { .. }
        | Statement::ExportAll { .. }
        | Statement::Import { .. }
        | Statement::Empty => {}
    }
}

/// Returns true if any expression within `stmts` references the identifier
/// `name`. Used to decide whether a function frame needs an `arguments` object.
///
/// Deliberately over-approximates: nested (non-arrow) function bodies are
/// included in the scan even though their `name` references bind to their own
/// frame. That only causes an unneeded object to be built — never a missing
/// one — so callers remain correct.
/// Whether a member's *computed name* (not its body) references `name`.
fn member_name_references(member: &ClassMember, name: &str) -> bool {
    let key = match member {
        ClassMember::Method { name, .. }
        | ClassMember::Field { name, .. }
        | ClassMember::Getter { name, .. }
        | ClassMember::Setter { name, .. } => name,
        ClassMember::StaticBlock { .. } => return false,
    };
    matches!(key, MemberName::Computed(e) if expr_references(e, name))
}

pub(crate) fn stmts_need_arguments(stmts: &[Statement]) -> bool {
    // Direct eval may inspect the arguments binding even without a static reference.
    stmts_reference(stmts, "arguments") || stmts_reference(stmts, "eval")
}

pub fn stmts_reference(stmts: &[Statement], name: &str) -> bool {
    stmts.iter().any(|s| stmt_references(s, name))
}

/// `stmts_reference` for an arrow-function body (expression or block).
pub fn arrow_body_references(body: &ExprOrBlock, name: &str) -> bool {
    match body {
        ExprOrBlock::Expr(e) => expr_references(e, name),
        ExprOrBlock::Block(s) => stmts_reference(s, name),
    }
}

/// Whether a classic `for` loop creates a callable that may retain one of its
/// `let` bindings after the iteration. Such loops need a fresh environment per
/// iteration; loops without a capturing function can update the binding in
/// place.
///
/// This deliberately over-approximates shadowing inside nested functions. A
/// false positive only keeps the slower environment-copy path; a false
/// negative would change the values observed by closures.
pub fn for_loop_captures_bindings(
    init: Option<&ForInit>,
    test: Option<&Expr>,
    update: Option<&Expr>,
    body: &[Statement],
    names: &[String],
) -> bool {
    names.iter().any(|name| {
        init.is_some_and(|init| for_init_captures(init, name))
            || test.is_some_and(|expr| expr_captures_identifier(expr, name))
            || update.is_some_and(|expr| expr_captures_identifier(expr, name))
            || statements_capture_identifier(body, name)
    })
}

fn for_binding_captures(binding: &ForBinding, name: &str) -> bool {
    match binding {
        ForBinding::Assignment(target) => expr_captures_identifier(target, name),
        ForBinding::Declaration {
            pattern,
            initializer,
            ..
        } => {
            pattern_captures_identifier(pattern, name)
                || initializer
                    .as_deref()
                    .is_some_and(|value| expr_captures_identifier(value, name))
        }
    }
}
fn for_binding_references(binding: &ForBinding, name: &str) -> bool {
    match binding {
        ForBinding::Assignment(target) => expr_references(target, name),
        ForBinding::Declaration {
            pattern,
            initializer,
            ..
        } => {
            pattern_references(pattern, name)
                || initializer
                    .as_deref()
                    .is_some_and(|value| expr_references(value, name))
        }
    }
}

fn for_init_captures(init: &ForInit, name: &str) -> bool {
    match init {
        ForInit::Var { decls, .. } => decls.iter().any(|(_, expr)| {
            expr.as_ref()
                .is_some_and(|expr| expr_captures_identifier(expr, name))
        }),
        ForInit::Pattern {
            pattern,
            init,
            trailing,
            ..
        } => {
            pattern_captures_identifier(pattern, name)
                || expr_captures_identifier(init, name)
                || trailing.iter().any(|(_, expr)| {
                    expr.as_ref()
                        .is_some_and(|expr| expr_captures_identifier(expr, name))
                })
        }
        ForInit::Expr(expr) => expr_captures_identifier(expr, name),
    }
}

pub(crate) fn statements_capture_identifier(stmts: &[Statement], name: &str) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Statement::Expr(expr) => expr_captures_identifier(expr, name),
        Statement::VarDecl {
            init,
            destructuring,
            ..
        } => {
            init.as_ref()
                .is_some_and(|expr| expr_captures_identifier(expr, name))
                || destructuring
                    .as_ref()
                    .is_some_and(|pattern| pattern_captures_identifier(pattern, name))
        }
        Statement::FnDecl { body, .. } => stmts_reference(body, name),
        Statement::ClassDecl {
            superclass, body, ..
        } => {
            superclass
                .as_ref()
                .is_some_and(|expr| expr_captures_identifier(expr, name))
                || class_members_capture_identifier(body, name)
        }
        Statement::Return(expr) => expr
            .as_ref()
            .is_some_and(|expr| expr_captures_identifier(expr, name)),
        Statement::If { test, then, else_ } => {
            expr_captures_identifier(test, name)
                || statements_capture_identifier(then, name)
                || else_
                    .as_ref()
                    .is_some_and(|stmts| statements_capture_identifier(stmts, name))
        }
        Statement::With { object: test, body }
        | Statement::While { test, body }
        | Statement::DoWhile { test, body } => {
            expr_captures_identifier(test, name) || statements_capture_identifier(body, name)
        }
        Statement::For {
            init,
            test,
            update,
            body,
        } => {
            init.as_deref()
                .is_some_and(|init| for_init_captures(init, name))
                || test
                    .as_deref()
                    .is_some_and(|expr| expr_captures_identifier(expr, name))
                || update
                    .as_deref()
                    .is_some_and(|expr| expr_captures_identifier(expr, name))
                || statements_capture_identifier(body, name)
        }
        Statement::ForIn { binding, obj, body } => {
            for_binding_captures(binding, name)
                || expr_captures_identifier(obj, name)
                || statements_capture_identifier(body, name)
        }
        Statement::ForOf {
            binding,
            iter,
            body,
            ..
        } => {
            for_binding_captures(binding, name)
                || expr_captures_identifier(iter, name)
                || statements_capture_identifier(body, name)
        }
        Statement::ResourceForOf { iter, body, .. } => {
            expr_captures_identifier(iter, name) || statements_capture_identifier(body, name)
        }
        Statement::ClassInitialization { fields, .. } => {
            statements_capture_identifier(fields, name)
        }
        Statement::ParameterInitialization {
            initializers,
            fields,
            ..
        } => {
            statements_capture_identifier(initializers, name)
                || statements_capture_identifier(fields, name)
        }
        Statement::Block(stmts)
        | Statement::Declarations(stmts)
        | Statement::ResourceDeclaration {
            declarations: stmts,
            ..
        } => statements_capture_identifier(stmts, name),
        Statement::Labeled { body, .. } => {
            statements_capture_identifier(std::slice::from_ref(body.as_ref()), name)
        }
        Statement::Throw(expr) | Statement::ExportDefault(expr) => {
            expr_captures_identifier(expr, name)
        }
        Statement::Try {
            body,
            catch,
            finally,
        } => {
            statements_capture_identifier(body, name)
                || catch.as_ref().is_some_and(|(pattern, stmts)| {
                    pattern
                        .as_ref()
                        .is_some_and(|pattern| pattern_captures_identifier(pattern, name))
                        || statements_capture_identifier(stmts, name)
                })
                || finally
                    .as_ref()
                    .is_some_and(|stmts| statements_capture_identifier(stmts, name))
        }
        Statement::Switch { disc, cases } => {
            expr_captures_identifier(disc, name)
                || cases.iter().any(|case| {
                    case.test
                        .as_ref()
                        .is_some_and(|expr| expr_captures_identifier(expr, name))
                        || statements_capture_identifier(&case.body, name)
                })
        }
        Statement::Break
        | Statement::Continue
        | Statement::LabeledBreak(_)
        | Statement::LabeledContinue(_)
        | Statement::ExportNamed { .. }
        | Statement::ExportAll { .. }
        | Statement::Import { .. }
        | Statement::Empty => false,
    })
}

pub(crate) fn expr_captures_identifier(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::LegacyLiteral(inner) | Expr::Parenthesized(inner) => {
            expr_captures_identifier(inner, name)
        }
        Expr::ArrowFn { body, .. } => arrow_body_references(body, name),
        Expr::FnExpr { body, .. } => stmts_reference(body, name),
        Expr::ClassExpr {
            superclass, body, ..
        } => {
            superclass
                .as_deref()
                .is_some_and(|expr| expr_captures_identifier(expr, name))
                || class_members_capture_identifier(body, name)
        }
        Expr::Array { items, .. } => items
            .iter()
            .any(|expr| expr_captures_identifier(expr, name)),
        Expr::Object { props, .. } => props.iter().any(|prop| match prop {
            ObjectProp::Shorthand(_) => false,
            ObjectProp::CoverInitializedName {
                initializer: value, ..
            }
            | ObjectProp::KeyValue(_, value)
            | ObjectProp::Spread(value) => expr_captures_identifier(value, name),
            ObjectProp::Computed(key, value) => {
                expr_captures_identifier(key, name) || expr_captures_identifier(value, name)
            }
            ObjectProp::ComputedMethod { key, body, .. } => {
                expr_captures_identifier(key, name) || stmts_reference(body, name)
            }
            ObjectProp::Method { body, .. }
            | ObjectProp::Getter { body, .. }
            | ObjectProp::Setter { body, .. } => stmts_reference(body, name),
        }),
        Expr::Binary { left, right, .. } => {
            expr_captures_identifier(left, name) || expr_captures_identifier(right, name)
        }
        Expr::Unary { operand, .. } | Expr::Spread(operand) | Expr::Await(operand) => {
            expr_captures_identifier(operand, name)
        }
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            expr_captures_identifier(callee, name)
                || args.iter().any(|arg| expr_captures_identifier(arg, name))
        }
        Expr::Member {
            object, property, ..
        }
        | Expr::OptionalChain {
            object, property, ..
        } => expr_captures_identifier(object, name) || expr_captures_identifier(property, name),
        Expr::TaggedTemplate { tag, exprs, .. } => {
            expr_captures_identifier(tag, name)
                || exprs
                    .iter()
                    .any(|expr| expr_captures_identifier(expr, name))
        }
        Expr::Assignment { target, value, .. } | Expr::LogicalAssignment { target, value, .. } => {
            expr_captures_identifier(target, name) || expr_captures_identifier(value, name)
        }
        Expr::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expr_captures_identifier(test, name)
                || expr_captures_identifier(consequent, name)
                || expr_captures_identifier(alternate, name)
        }
        Expr::DynamicImport {
            specifier, options, ..
        } => {
            expr_captures_identifier(specifier, name)
                || options
                    .as_deref()
                    .is_some_and(|expr| expr_captures_identifier(expr, name))
        }
        Expr::YieldFrom(specifier) => expr_captures_identifier(specifier, name),
        Expr::Template { exprs, .. } => exprs
            .iter()
            .any(|expr| expr_captures_identifier(expr, name)),
        Expr::Yield(expr) => expr
            .as_deref()
            .is_some_and(|expr| expr_captures_identifier(expr, name)),
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
        | Expr::NewTarget => false,
    }
}

fn class_members_capture_identifier(members: &[ClassMember], name: &str) -> bool {
    members.iter().any(|member| match member {
        ClassMember::Method {
            body, name: key, ..
        }
        | ClassMember::Getter {
            body, name: key, ..
        }
        | ClassMember::Setter {
            body, name: key, ..
        } => {
            matches!(key, MemberName::Computed(expr) if expr_captures_identifier(expr, name))
                || stmts_reference(body, name)
        }
        ClassMember::Field {
            name: key, init, ..
        } => {
            matches!(key, MemberName::Computed(expr) if expr_captures_identifier(expr, name))
                || init.as_ref().is_some_and(|expr| {
                    // Initializers execute in a child of the class scope,
                    // including static fields through the shared class builder.
                    expr_references(expr, name)
                })
        }
        // Static blocks run in a fresh child of the defining scope, so any
        // reference captures through the chain.
        ClassMember::StaticBlock { body } => stmts_reference(body, name),
    })
}

fn pattern_captures_identifier(pattern: &Pattern, name: &str) -> bool {
    match pattern {
        Pattern::Elision | Pattern::Ident(_) => false,
        Pattern::Array(items) => items
            .iter()
            .any(|item| pattern_captures_identifier(item, name)),
        Pattern::Object(items) => items.iter().any(|(key, value)| {
            matches!(key, PatternKey::Computed(expr) if expr_captures_identifier(expr, name))
                || value
                    .as_ref()
                    .is_some_and(|pattern| pattern_captures_identifier(pattern, name))
        }),
        Pattern::Rest(inner) => pattern_captures_identifier(inner, name),
        Pattern::Default(inner, default) => {
            pattern_captures_identifier(inner, name) || expr_captures_identifier(default, name)
        }
        Pattern::Member {
            object, property, ..
        } => expr_captures_identifier(object, name) || expr_captures_identifier(property, name),
    }
}

fn stmt_references(s: &Statement, name: &str) -> bool {
    match s {
        Statement::Expr(e) => expr_references(e, name),
        Statement::VarDecl {
            init,
            destructuring,
            ..
        } => {
            init.as_ref()
                .map(|e| expr_references(e, name))
                .unwrap_or(false)
                || destructuring
                    .as_ref()
                    .map(|p| pattern_references(p, name))
                    .unwrap_or(false)
        }
        Statement::FnDecl { body, .. } => stmts_reference(body, name),
        Statement::ClassDecl {
            superclass, body, ..
        } => {
            superclass
                .as_ref()
                .map(|e| expr_references(e, name))
                .unwrap_or(false)
                || body.iter().any(|m| {
                    member_name_references(m, name)
                        || match m {
                            ClassMember::Method { body, .. } => stmts_reference(body, name),
                            ClassMember::Field { init, .. } => init
                                .as_ref()
                                .map(|e| expr_references(e, name))
                                .unwrap_or(false),
                            ClassMember::Getter { body, .. } => stmts_reference(body, name),
                            ClassMember::Setter { body, .. } => stmts_reference(body, name),
                            ClassMember::StaticBlock { body } => stmts_reference(body, name),
                        }
                })
        }
        Statement::Return(e) => e
            .as_ref()
            .map(|e| expr_references(e, name))
            .unwrap_or(false),
        Statement::If { test, then, else_ } => {
            expr_references(test, name)
                || stmts_reference(then, name)
                || else_
                    .as_ref()
                    .map(|b| stmts_reference(b, name))
                    .unwrap_or(false)
        }
        Statement::With { object: test, body }
        | Statement::While { test, body }
        | Statement::DoWhile { test, body } => {
            expr_references(test, name) || stmts_reference(body, name)
        }
        Statement::For {
            init,
            test,
            update,
            body,
        } => {
            init.as_ref()
                .map(|i| match i.as_ref() {
                    ForInit::Var { decls, .. } => decls.iter().any(|(_, e)| {
                        e.as_ref()
                            .map(|e| expr_references(e, name))
                            .unwrap_or(false)
                    }),
                    ForInit::Pattern {
                        pattern,
                        init,
                        trailing,
                        ..
                    } => {
                        pattern_references(pattern, name)
                            || expr_references(init, name)
                            || trailing.iter().any(|(_, e)| {
                                e.as_ref()
                                    .map(|e| expr_references(e, name))
                                    .unwrap_or(false)
                            })
                    }
                    ForInit::Expr(e) => expr_references(e, name),
                })
                .unwrap_or(false)
                || test
                    .as_ref()
                    .map(|e| expr_references(e, name))
                    .unwrap_or(false)
                || update
                    .as_ref()
                    .map(|e| expr_references(e, name))
                    .unwrap_or(false)
                || stmts_reference(body, name)
        }
        Statement::ForIn { binding, obj, body } => {
            for_binding_references(binding, name)
                || expr_references(obj, name)
                || stmts_reference(body, name)
        }
        Statement::ForOf {
            binding,
            iter,
            body,
            ..
        } => {
            for_binding_references(binding, name)
                || expr_references(iter, name)
                || stmts_reference(body, name)
        }
        Statement::ResourceForOf { iter, body, .. } => {
            expr_references(iter, name) || stmts_reference(body, name)
        }
        Statement::ClassInitialization { fields, .. } => stmts_reference(fields, name),
        Statement::ParameterInitialization {
            initializers,
            fields,
            ..
        } => stmts_reference(initializers, name) || stmts_reference(fields, name),
        Statement::Block(b)
        | Statement::Declarations(b)
        | Statement::ResourceDeclaration {
            declarations: b, ..
        } => stmts_reference(b, name),
        Statement::Labeled { body, .. } => stmt_references(body, name),
        Statement::Throw(e) => expr_references(e, name),
        Statement::Try {
            body,
            catch,
            finally,
        } => {
            stmts_reference(body, name)
                || catch
                    .as_ref()
                    .map(|(pattern, b)| {
                        pattern
                            .as_ref()
                            .is_some_and(|pattern| pattern_references(pattern, name))
                            || stmts_reference(b, name)
                    })
                    .unwrap_or(false)
                || finally
                    .as_ref()
                    .map(|b| stmts_reference(b, name))
                    .unwrap_or(false)
        }
        Statement::Switch { disc, cases } => {
            expr_references(disc, name)
                || cases.iter().any(|c| {
                    c.test
                        .as_ref()
                        .map(|e| expr_references(e, name))
                        .unwrap_or(false)
                        || stmts_reference(&c.body, name)
                })
        }
        Statement::ExportDefault(e) => expr_references(e, name),
        Statement::Break
        | Statement::Continue
        | Statement::LabeledBreak(_)
        | Statement::LabeledContinue(_)
        | Statement::ExportNamed { .. }
        | Statement::ExportAll { .. }
        | Statement::Import { .. }
        | Statement::Empty => false,
    }
}

fn expr_references(e: &Expr, name: &str) -> bool {
    match e {
        Expr::LegacyLiteral(inner) | Expr::Parenthesized(inner) => expr_references(inner, name),
        Expr::Regex(_, _) | Expr::BigIntLiteral(_) => false,
        Expr::Identifier(n) => n == name,
        Expr::Array { items, .. } => items.iter().any(|x| expr_references(x, name)),
        Expr::Object { props, .. } => props.iter().any(|p| match p {
            ObjectProp::Shorthand(n) => n == name,
            ObjectProp::CoverInitializedName { initializer, .. } => {
                expr_references(initializer, name)
            }
            ObjectProp::KeyValue(_, v) => expr_references(v, name),
            ObjectProp::Computed(k, v) => expr_references(k, name) || expr_references(v, name),
            ObjectProp::ComputedMethod { key, body, .. } => {
                expr_references(key, name) || stmts_reference(body, name)
            }
            ObjectProp::Method { body, .. } => stmts_reference(body, name),
            ObjectProp::Getter { body, .. } => stmts_reference(body, name),
            ObjectProp::Setter { body, .. } => stmts_reference(body, name),
            ObjectProp::Spread(x) => expr_references(x, name),
        }),
        Expr::Binary { left, right, .. } => {
            expr_references(left, name) || expr_references(right, name)
        }
        Expr::Unary { operand, .. } => expr_references(operand, name),
        Expr::ClassExpr {
            superclass, body, ..
        } => {
            superclass
                .as_ref()
                .map(|e| expr_references(e, name))
                .unwrap_or(false)
                || body.iter().any(|m| {
                    member_name_references(m, name)
                        || match m {
                            ClassMember::Method { body, .. } => stmts_reference(body, name),
                            ClassMember::Field { init, .. } => init
                                .as_ref()
                                .map(|e| expr_references(e, name))
                                .unwrap_or(false),
                            ClassMember::Getter { body, .. } => stmts_reference(body, name),
                            ClassMember::Setter { body, .. } => stmts_reference(body, name),
                            ClassMember::StaticBlock { body } => stmts_reference(body, name),
                        }
                })
        }
        Expr::LogicalAssignment { target, value, .. } => {
            expr_references(target, name) || expr_references(value, name)
        }
        Expr::TaggedTemplate { tag, exprs, .. } => {
            expr_references(tag, name) || exprs.iter().any(|x| expr_references(x, name))
        }
        Expr::Call { callee, args } => {
            expr_references(callee, name) || args.iter().any(|a| expr_references(a, name))
        }
        Expr::Member {
            object, property, ..
        } => expr_references(object, name) || expr_references(property, name),
        Expr::OptionalChain {
            object, property, ..
        } => expr_references(object, name) || expr_references(property, name),
        Expr::Assignment { target, value, .. } => {
            expr_references(target, name) || expr_references(value, name)
        }
        Expr::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expr_references(test, name)
                || expr_references(consequent, name)
                || expr_references(alternate, name)
        }
        Expr::ArrowFn { body, .. } => match body.as_ref() {
            ExprOrBlock::Expr(x) => expr_references(x, name),
            ExprOrBlock::Block(s) => stmts_reference(s, name),
        },
        Expr::FnExpr { body, .. } => stmts_reference(body, name),
        Expr::New { callee, args } => {
            expr_references(callee, name) || args.iter().any(|a| expr_references(a, name))
        }
        Expr::Spread(x) => expr_references(x, name),
        Expr::Template { exprs, .. } => exprs.iter().any(|x| expr_references(x, name)),
        Expr::Await(x) => expr_references(x, name),
        Expr::YieldFrom(x) => expr_references(x, name),
        Expr::Yield(x) => x
            .as_ref()
            .map(|x| expr_references(x, name))
            .unwrap_or(false),
        Expr::Number(_)
        | Expr::String(_)
        | Expr::EscapedString(_)
        | Expr::Bool(_)
        | Expr::Null
        | Expr::Undefined
        | Expr::This
        | Expr::Super
        | Expr::ImportMeta
        | Expr::NewTarget => false,
        Expr::DynamicImport {
            specifier, options, ..
        } => {
            expr_references(specifier, name)
                || options
                    .as_deref()
                    .is_some_and(|expr| expr_references(expr, name))
        }
    }
}

/// Read an assignment target as a binding pattern: `[a, b]`, `({ x })`,
/// `[o.p]`, `[a = 1]`. Holes skip a position. `None` means the target is
/// not a valid pattern and the assignment fails at runtime.
pub fn expr_to_pattern(expr: &Expr) -> Option<Pattern> {
    Some(match expr {
        Expr::LegacyLiteral(inner) | Expr::Parenthesized(inner) => return expr_to_pattern(inner),
        Expr::Identifier(name) => Pattern::Ident(name.clone()),
        Expr::Member {
            object,
            property,
            computed,
        } => Pattern::Member {
            object: object.clone(),
            property: property.clone(),
            private: !computed
                && matches!(property.as_ref(), Expr::String(name) if name.to_key().starts_with('#')),
        },
        Expr::Array { items, .. } => Pattern::Array(
            items
                .iter()
                .map(|item| match item {
                    Expr::Spread(inner) => {
                        expr_to_pattern(inner).map(|p| Pattern::Rest(Box::new(p)))
                    }
                    // A hole (`[, a] = …`) skips a position.
                    Expr::Undefined => Some(Pattern::Elision),
                    other => expr_to_pattern(other),
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        Expr::Object { props, .. } => Pattern::Object(
            props
                .iter()
                .map(|prop| match prop {
                    ObjectProp::CoverInitializedName { name, initializer } => Some((
                        PatternKey::Name(name.clone()),
                        Some(Pattern::Default(
                            Box::new(Pattern::Ident(name.clone())),
                            Box::new(initializer.clone()),
                        )),
                    )),
                    ObjectProp::Shorthand(name) => Some((PatternKey::Name(name.clone()), None)),
                    ObjectProp::KeyValue(key, value) => {
                        Some((PatternKey::Name(key.clone()), Some(expr_to_pattern(value)?)))
                    }
                    ObjectProp::Spread(inner) => Some((
                        PatternKey::Name("...".to_string()),
                        Some(Pattern::Rest(Box::new(expr_to_pattern(inner)?))),
                    )),
                    ObjectProp::Computed(key, value) => Some((
                        PatternKey::Computed(key.clone()),
                        Some(expr_to_pattern(value)?),
                    )),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        // `[a = 1] = []` supplies a default.
        Expr::Assignment {
            target,
            op: AssignOp::Assign,
            value,
        } => Pattern::Default(
            Box::new(expr_to_pattern(target)?),
            Box::new(value.as_ref().clone()),
        ),
        _ => return None,
    })
}

fn pattern_references(p: &Pattern, name: &str) -> bool {
    match p {
        Pattern::Elision | Pattern::Ident(_) | Pattern::Rest(_) => false,
        Pattern::Member {
            object, property, ..
        } => expr_references(object, name) || expr_references(property, name),
        Pattern::Array(elems) => elems.iter().any(|e| pattern_references(e, name)),
        Pattern::Object(props) => props.iter().any(|(key, p)| {
            matches!(key, PatternKey::Computed(e) if expr_references(e, name))
                || p.as_ref()
                    .map(|p| pattern_references(p, name))
                    .unwrap_or(false)
        }),
        Pattern::Default(inner, default) => {
            pattern_references(inner, name) || expr_references(default, name)
        }
    }
}

/// ExpectedArgumentCount stops before the first default or rest parameter.
pub(crate) fn formal_parameter_length<S: AsRef<str>>(params: &[S], body: &[Statement]) -> usize {
    let initializers = match body
        .iter()
        .find(|stmt| !matches!(stmt, Statement::ClassInitialization { .. }))
    {
        Some(Statement::ParameterInitialization { initializers, .. }) => initializers.as_slice(),
        _ => &[],
    };
    params.iter().take_while(|parameter| {
        let name = parameter.as_ref();
        !name.starts_with("...") && !initializers.iter().any(|initializer| {
            matches!(initializer, Statement::If { then, .. } if matches!(then.first(), Some(Statement::Expr(Expr::Assignment { target, .. })) if matches!(target.as_ref(), Expr::Identifier(target) if target == name)))
        })
    }).count()
}
