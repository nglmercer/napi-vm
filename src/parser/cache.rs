//! Content-addressed parse cache: identical sources share one AST across
//! every interpreter in the process.
//!
//! Plugin modules are read from disk per load but change rarely; without a
//! cache every restart re-lexes and re-parses every module, and the static
//! import scan parses each module a second time. The cache keys on source
//! length plus two 64-bit hashes, so a stale entry needs a 128-bit
//! collision. Entries are `Arc`-shared ASTs (pure data, safe to share
//! across threads); failures are never cached. The table is bounded and
//! clears wholesale when full — simple and starvation-free, since hot
//! entries re-populate on next use. A poisoned lock fails open to a fresh
//! parse rather than poisoning execution.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::{Parser, Statement};
use crate::error::VmErr;
use crate::lexer::Lexer;

/// Parse failure detail, so call sites keep their exact error mapping.
/// `message` is `ParseError::to_string()`: identical to what an uncached
/// parse reports. `depth_exceeded` lets callers upgrade to `RangeError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachedParseError {
    pub message: String,
    pub depth_exceeded: bool,
}

impl CachedParseError {
    /// The error an uncached parse reports for this failure: depth
    /// overruns upgrade to `RangeError`, everything else keeps the
    /// parser message verbatim.
    pub(crate) fn into_vm_err(self) -> VmErr {
        if self.depth_exceeded {
            VmErr::Msg("RangeError: Maximum parse depth exceeded".to_string())
        } else {
            VmErr::Msg(self.message)
        }
    }
}

const MAX_CACHED_PROGRAMS: usize = 1024;

/// Source key → shared AST for every identical source in the process.
type ParseCacheMap = HashMap<(u64, u64, u64), Arc<Vec<Statement>>>;

static PARSE_CACHE: OnceLock<Mutex<ParseCacheMap>> = OnceLock::new();

fn cache() -> &'static Mutex<ParseCacheMap> {
    PARSE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn source_key(source: &str) -> (u64, u64, u64) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut first = DefaultHasher::new();
    0u8.hash(&mut first);
    source.hash(&mut first);
    let mut second = DefaultHasher::new();
    1u8.hash(&mut second);
    source.hash(&mut second);
    (source.len() as u64, first.finish(), second.finish())
}

/// Lex + parse `source`, sharing the AST with every identical source in
/// the process. Hit: no lexer, no parser. Miss: parse once and store.
/// Errors are re-parsed on every call (cold path, never cached).
pub(crate) fn parse_cached(source: &str) -> Result<Arc<Vec<Statement>>, CachedParseError> {
    let key = source_key(source);
    if let Ok(cache) = cache().lock()
        && let Some(hit) = cache.get(&key)
    {
        return Ok(Arc::clone(hit));
    }
    let tokens = Lexer::new(source).tokenize_with_spans();
    let mut parser = Parser::new_with_spans(tokens);
    let statements = match parser.parse_program() {
        Ok(statements) => statements,
        Err(error) => {
            return Err(CachedParseError {
                message: error.to_string(),
                depth_exceeded: parser.depth_exceeded,
            });
        }
    };
    let shared = Arc::new(statements);
    if let Ok(mut cache) = cache().lock() {
        if cache.len() >= MAX_CACHED_PROGRAMS {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&shared));
    }
    Ok(shared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::PARSE_PROGRAM_COUNT;

    fn parse_count() -> u64 {
        PARSE_PROGRAM_COUNT.with(|count| count.get())
    }

    #[test]
    fn repeated_compiles_share_one_parse() {
        let source = "const cache_shared_marker_a = 40 + 2;";
        let before = parse_count();
        let first = parse_cached(source).unwrap();
        let second = parse_cached(source).unwrap();
        assert_eq!(parse_count() - before, 1);
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn distinct_sources_parse_independently() {
        let before = parse_count();
        parse_cached("const cache_distinct_marker_b = 1;").unwrap();
        parse_cached("const cache_distinct_marker_c = 2;").unwrap();
        assert_eq!(parse_count() - before, 2);
    }

    #[test]
    fn failed_parses_are_not_cached() {
        let source = "const cache_broken_marker_d = {{{;";
        let before = parse_count();
        assert!(parse_cached(source).is_err());
        assert!(parse_cached(source).is_err());
        assert_eq!(parse_count() - before, 2);
    }

    #[test]
    fn depth_failure_detail_survives_the_cache_boundary() {
        let deep = "(".repeat(200) + &")".repeat(200);
        // Debug parser frames are fat: 65 guarded levels exceed the default
        // test-thread stack, so run the probe on a roomy thread.
        let failure = std::thread::Builder::new()
            .name("cache-depth-probe".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || parse_cached(&deep).unwrap_err())
            .unwrap()
            .join()
            .unwrap();
        assert!(failure.depth_exceeded, "{}", failure.message);
    }

    #[test]
    fn prepared_program_executes_repeatedly_with_one_parse() {
        use crate::interpreter::Interpreter;
        use crate::value::Value;
        let source = "var cache_exec_marker_p = (typeof cache_exec_marker_p === 'undefined' ? 1 : cache_exec_marker_p + 1); cache_exec_marker_p;";
        let program = Interpreter::compile(source).unwrap();
        let before = parse_count();
        let mut interp = Interpreter::new();
        let first = interp.execute(&program).unwrap();
        let second = interp.execute(&program).unwrap();
        assert_eq!(parse_count() - before, 0);
        assert!(matches!(first, Value::Number(n) if n == 1.0));
        assert!(matches!(second, Value::Number(n) if n == 2.0));
    }

    #[test]
    fn prepared_errors_match_eval_source() {
        use crate::interpreter::Interpreter;
        let source = "const cache_err_marker_q = {{{;";
        let program_err = match Interpreter::compile(source) {
            Ok(_) => panic!("expected compile failure"),
            Err(error) => error.to_string(),
        };
        let mut interp = Interpreter::new();
        let eval_err = match interp.eval_source(source) {
            Ok(_) => panic!("expected eval failure"),
            Err(error) => error.to_string(),
        };
        assert_eq!(program_err, eval_err);
    }
}
