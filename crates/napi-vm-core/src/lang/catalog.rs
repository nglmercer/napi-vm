//! Static knowledge about the interpreter's built-in globals. This is the
//! single source of truth shared by every frontend (WASM playground, LSP,
//! native GUIs) — completions are never re-derived per frontend.
//!
//! The live interpreter can also be probed at runtime (see the WASM layer),
//! but this catalog is what makes completion work on code that does not yet
//! parse or run, which is the common case while typing.

/// Top-level globals present in a fresh VM.
pub const GLOBALS: &[&str] = &[
    "Math",
    "JSON",
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "Promise",
    "Date",
    "Symbol",
    "console",
    "Error",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "ReferenceError",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "undefined",
    "NaN",
    "Infinity",
    "globalThis",
    "window",
    "self",
];

/// Language keywords offered during bare-identifier completion.
pub const KEYWORDS: &[&str] = &[
    "var",
    "let",
    "const",
    "function",
    "return",
    "if",
    "else",
    "for",
    "while",
    "do",
    "break",
    "continue",
    "switch",
    "case",
    "default",
    "try",
    "catch",
    "finally",
    "throw",
    "new",
    "typeof",
    "instanceof",
    "class",
    "extends",
    "super",
    "import",
    "export",
    "from",
    "async",
    "await",
    "yield",
    "of",
    "in",
    "this",
    "null",
    "undefined",
    "true",
    "false",
    "void",
    "delete",
    "static",
];

/// Type information for native globals and their native members. This is
/// intentionally separate from the document model so the same description can
/// later be consumed by completion, hover, diagnostics, and an LSP adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinType {
    Unknown,
    Number,
    String,
    Boolean,
    Undefined,
    Function { result: &'static str },
    NativeObject(&'static str),
}

/// Static type of a built-in global. Local declarations still take precedence
/// over this catalog in the document analyzer.
pub fn builtin_global_type(name: &str) -> Option<BuiltinType> {
    match name {
        "Math" => Some(BuiltinType::NativeObject("Math")),
        "JSON" => Some(BuiltinType::NativeObject("JSON")),
        "Object" => Some(BuiltinType::NativeObject("Object")),
        "Array" => Some(BuiltinType::NativeObject("Array")),
        "String" => Some(BuiltinType::NativeObject("String")),
        "Number" => Some(BuiltinType::NativeObject("Number")),
        "Boolean" => Some(BuiltinType::NativeObject("Boolean")),
        "Promise" => Some(BuiltinType::NativeObject("Promise")),
        "Date" => Some(BuiltinType::NativeObject("Date")),
        "Symbol" => Some(BuiltinType::NativeObject("Symbol")),
        "console" => Some(BuiltinType::NativeObject("console")),
        "NaN" | "Infinity" => Some(BuiltinType::Number),
        "undefined" => Some(BuiltinType::Undefined),
        "parseInt" | "parseFloat" => Some(BuiltinType::Function { result: "number" }),
        "isNaN" | "isFinite" => Some(BuiltinType::Function { result: "boolean" }),
        _ => None,
    }
}

/// Static type of a member on a native global or native object.
pub fn builtin_member_type(receiver: &str, member: &str) -> Option<BuiltinType> {
    match (receiver, member) {
        ("Date", "now" | "parse" | "UTC") => Some(BuiltinType::Function { result: "number" }),
        _ => None,
    }
}

/// Type information for native prototype members. Names remain in the same
/// catalog as completion so hover does not report an implemented method as
/// `unknown`.
pub fn prototype_member_type(kind: ProtoKind, member: &str) -> Option<BuiltinType> {
    let result = match kind {
        ProtoKind::String => match member {
            "length" => return Some(BuiltinType::Number),
            "toUpperCase" | "toLowerCase" | "slice" | "substring" | "charAt" | "repeat"
            | "trim" | "replace" => "string",
            "includes" | "startsWith" | "endsWith" => "boolean",
            "indexOf" => "number",
            "split" => "unknown",
            _ => return None,
        },
        ProtoKind::Number => match member {
            "toFixed" | "toString" => "string",
            "valueOf" => "number",
            _ => return None,
        },
        ProtoKind::Array => match member {
            "length" => return Some(BuiltinType::Number),
            _ => "unknown",
        },
        ProtoKind::Promise => match member {
            "then" | "catch" | "finally" => "unknown",
            _ => return None,
        },
    };
    Some(BuiltinType::Function { result })
}

/// Members of a named built-in global, keyed by the receiver that precedes the
/// dot (e.g. `Math` → `["abs", "floor", …]`).
pub fn builtin_members(receiver: &str) -> Option<&'static [&'static str]> {
    Some(match receiver {
        "Math" => &[
            "abs", "floor", "ceil", "round", "sqrt", "cbrt", "pow", "min", "max", "random",
            "trunc", "sign", "log", "log2", "log10", "exp", "sin", "cos", "tan", "hypot", "PI",
            "E", "LN2", "LN10", "LOG2E", "LOG10E", "SQRT1_2", "SQRT2",
        ],
        "JSON" => &["parse", "stringify"],
        "console" => &["log", "info", "debug", "error", "warn", "dir"],
        "Object" => &["keys", "values", "entries", "assign"],
        "Array" => &["isArray", "from", "of"],
        "Number" => &[
            "isNaN",
            "isFinite",
            "isInteger",
            "MAX_SAFE_INTEGER",
            "MIN_SAFE_INTEGER",
            "EPSILON",
        ],
        "Promise" => &["resolve", "reject", "all", "race", "allSettled", "any"],
        "Date" => &["now", "parse", "UTC"],
        "Symbol" => &[
            "iterator",
            "asyncIterator",
            "toStringTag",
            "hasInstance",
            "for",
            "keyFor",
        ],
        "String" => &["fromCharCode"],
        _ => return None,
    })
}

/// Prototype methods offered for a value of a known built-in type. Used when
/// the receiver is a literal (`[1,2].`, `"x".`) or a variable whose initializer
/// reveals its type — cases where `Object.keys`-style own-key enumeration would
/// miss the prototype chain.
pub fn prototype_members(kind: ProtoKind) -> &'static [&'static str] {
    match kind {
        ProtoKind::Array => &[
            "length",
            "map",
            "filter",
            "reduce",
            "reduceRight",
            "forEach",
            "find",
            "some",
            "every",
            "push",
            "pop",
            "shift",
            "unshift",
            "join",
            "slice",
            "splice",
            "concat",
            "reverse",
            "sort",
            "flat",
            "flatMap",
            "indexOf",
            "includes",
        ],
        ProtoKind::String => &[
            "length",
            "toUpperCase",
            "toLowerCase",
            "slice",
            "substring",
            "split",
            "includes",
            "indexOf",
            "trim",
            "replace",
            "charAt",
            "startsWith",
            "endsWith",
            "repeat",
        ],
        ProtoKind::Number => &["toFixed", "toString", "valueOf"],
        ProtoKind::Promise => &["then", "catch", "finally"],
    }
}

/// A coarse built-in type used to select prototype members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoKind {
    Array,
    String,
    Number,
    Promise,
}
