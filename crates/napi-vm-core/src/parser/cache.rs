//! Content-addressed parse cache: identical sources share one AST across
//! every interpreter in the process.
//!
//! Entries compare exact source contents, retain at most 1024 programs and
//! 8 MiB of source, and evict least-recently-used entries with bounded periodic recency cleanup. Failures are not
//! cached. A poisoned lock fails open to a fresh parse.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::{ParseGoal, Parser, Statement};
use crate::error::VmErr;
use crate::lexer::Lexer;

/// Parse failure detail, so call sites keep their exact error mapping.
/// `message` is `ParseError::to_string()`: identical to what an uncached
/// parse reports. `depth_exceeded` lets callers upgrade to `RangeError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedParseError {
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
const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;

struct ParseCacheEntry {
    source: Arc<str>,
    statements: Arc<Vec<Statement>>,
    generation: u64,
}
/// Exact-source LRU. Hash collisions are resolved by HashMap's equality check.
#[derive(Default)]
struct ParseCache<S = std::collections::hash_map::RandomState> {
    entries: HashMap<Arc<str>, ParseCacheEntry, S>,
    order: std::collections::VecDeque<(Arc<str>, u64)>,
    source_bytes: usize,
}
impl<S: std::hash::BuildHasher> ParseCache<S> {
    fn get(&mut self, source: &str) -> Option<Arc<Vec<Statement>>> {
        let entry = self.entries.get_mut(source)?;
        let result = entry.statements.clone();
        entry.generation = entry.generation.wrapping_add(1);
        self.order
            .push_back((entry.source.clone(), entry.generation));
        // Hits are O(1); compact only after a bounded batch of accesses.
        // At most two records per permitted entry survive between cleanups.
        if self.order.len() > 2 * MAX_CACHED_PROGRAMS {
            self.order.retain(|(key, generation)| {
                self.entries
                    .get(key)
                    .is_some_and(|entry| entry.generation == *generation)
            });
        }
        Some(result)
    }
    fn insert(&mut self, source: &str, statements: Arc<Vec<Statement>>) {
        if source.len() > MAX_SOURCE_BYTES || self.entries.contains_key(source) {
            return;
        }
        while self.entries.len() >= MAX_CACHED_PROGRAMS
            || self.source_bytes + source.len() > MAX_SOURCE_BYTES
        {
            let Some((key, generation)) = self.order.pop_front() else {
                break;
            };
            if self
                .entries
                .get(&key)
                .is_some_and(|entry| entry.generation == generation)
            {
                self.entries.remove(&key);
                self.source_bytes -= key.len();
            }
        }
        let source: Arc<str> = source.into();
        self.source_bytes += source.len();
        self.order.push_back((source.clone(), 0));
        self.entries.insert(
            source.clone(),
            ParseCacheEntry {
                source,
                statements,
                generation: 0,
            },
        );
    }
}
static PARSE_CACHE: OnceLock<Mutex<ParseCache>> = OnceLock::new();
fn cache() -> &'static Mutex<ParseCache> {
    PARSE_CACHE.get_or_init(|| Mutex::new(ParseCache::default()))
}

/// Lex + parse `source`, sharing the AST with every identical source in
/// the process. Hit: no lexer, no parser. Miss: parse once and store.
/// Errors are re-parsed on every call (cold path, never cached).
#[doc(hidden)]
pub fn parse_cached(source: &str) -> Result<Arc<Vec<Statement>>, CachedParseError> {
    parse_cached_with_goal(source, ParseGoal::Auto)
}

static SCRIPT_PARSE_CACHE: OnceLock<Mutex<ParseCache>> = OnceLock::new();
static MODULE_PARSE_CACHE: OnceLock<Mutex<ParseCache>> = OnceLock::new();

/// Identical source text under different grammar goals never shares a cached AST.
pub fn parse_cached_with_goal(
    source: &str,
    goal: ParseGoal,
) -> Result<Arc<Vec<Statement>>, CachedParseError> {
    let goal_cache = match goal {
        ParseGoal::Auto => cache(),
        ParseGoal::Script => SCRIPT_PARSE_CACHE.get_or_init(|| Mutex::new(ParseCache::default())),
        ParseGoal::Module => MODULE_PARSE_CACHE.get_or_init(|| Mutex::new(ParseCache::default())),
    };
    if let Ok(mut cache) = goal_cache.lock()
        && let Some(hit) = cache.get(source)
    {
        return Ok(hit);
    }
    let tokens = Lexer::new(source).tokenize_with_spans();
    let mut parser = Parser::new_with_spans(tokens);
    let statements = match parser.parse_program_with_goal(goal) {
        Ok(statements) => statements,
        Err(error) => {
            return Err(CachedParseError {
                message: error.to_string(),
                depth_exceeded: parser.depth_exceeded,
            });
        }
    };
    let shared = Arc::new(statements);
    if let Ok(mut cache) = goal_cache.lock() {
        cache.insert(source, Arc::clone(&shared));
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

    #[derive(Default)]
    struct CollidingHasher;
    impl std::hash::Hasher for CollidingHasher {
        fn finish(&self) -> u64 {
            0
        }
        fn write(&mut self, _: &[u8]) {}
    }
    type CollisionBuilder = std::hash::BuildHasherDefault<CollidingHasher>;
    #[test]
    fn exact_contents_resolve_forced_hash_collisions() {
        let mut cache = ParseCache::<CollisionBuilder>::default();
        let one = parse_cached("1;").unwrap();
        let two = parse_cached("2;").unwrap();
        cache.insert("1;", one.clone());
        cache.insert("2;", two.clone());
        assert!(Arc::ptr_eq(&cache.get("1;").unwrap(), &one));
        assert!(Arc::ptr_eq(&cache.get("2;").unwrap(), &two));
        assert!(cache.get("3;").is_none());
    }
    #[test]
    fn eviction_preserves_recent_entries_and_bounds_bytes() {
        let mut cache = ParseCache::<std::collections::hash_map::RandomState>::default();
        let ast = Arc::new(Vec::new());
        for i in 0..MAX_CACHED_PROGRAMS {
            cache.insert(&format!("{i};"), ast.clone());
        }
        cache.get("0;");
        cache.insert("overflow;", ast.clone());
        assert!(cache.get("0;").is_some());
        assert!(cache.get("1;").is_none());
        assert_eq!(cache.entries.len(), MAX_CACHED_PROGRAMS);
        cache.insert(&" ".repeat(MAX_SOURCE_BYTES + 1), ast);
        assert!(cache.source_bytes <= MAX_SOURCE_BYTES);
    }

    #[test]
    fn hot_hits_keep_recency_storage_bounded() {
        let mut cache = ParseCache::<std::collections::hash_map::RandomState>::default();
        let ast = Arc::new(Vec::new());
        for i in 0..MAX_CACHED_PROGRAMS {
            cache.insert(&format!("{i};"), ast.clone());
        }
        for _ in 0..10_000 {
            assert!(Arc::ptr_eq(&cache.get("0;").unwrap(), &ast));
            assert!(cache.order.len() <= 2 * MAX_CACHED_PROGRAMS);
        }
        cache.insert("overflow;", ast);
        assert!(cache.get("0;").is_some());
        assert!(cache.get("1;").is_none());
        assert_eq!(cache.entries.len(), MAX_CACHED_PROGRAMS);
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
