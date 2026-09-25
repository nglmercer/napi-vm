//! Object shapes and property inline caches.
//!
//! Every ordinary object stores its properties as an insertion-ordered slot
//! vector; a *shape* is the canonical shared descriptor of one key layout.
//! Objects built by the same sequence of property adds — every instance of
//! a class, every object literal in a loop — share one shape, so a lookup
//! compiles to an index instead of a scan, and a bytecode property site
//! caches the shape it last saw instead of looking anything up.
//!
//! Shapes describe keys only, never attributes or values. Attributes stay
//! per-object (in [`ObjectMeta`]); two objects with the same keys but
//! different writability share a shape, which is what keeps the tree small.
//! A shape is also never authoritative: [`ObjectCell`] keeps its slot vector
//! as the source of truth and treats its shape as a cache. Every indexed
//! access verifies the key at the cached index, so a mutation that bypasses
//! the shape-maintaining methods only costs a rebuild, never a wrong value.
//!
//! The tree is thread-local. Transitions memoize on the parent node, so the
//! same add sequence from the root always reaches the same node no matter
//! which interpreter or realm built it; deletes rebuild by replaying the
//! surviving keys from the root, which canonicalizes them the same way.
//! Shape ids come from a thread-local counter and are never reused, so an
//! inline-cache guard comparing them cannot alias across layouts.
//!
//! [`ObjectCell`]: crate::value::ObjectCell
//! [`ObjectMeta`]: crate::value::ObjectMeta

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Canonical descriptor of one object key layout: insertion-ordered keys
/// plus a key-to-slot index, with memoized add transitions to children.
#[derive(Debug)]
pub(crate) struct Shape {
    /// Unique within this thread; never reused. Inline caches guard on it.
    pub id: u32,
    /// Own keys in insertion order, parallel to the object's slot vector.
    keys: Vec<Rc<str>>,
    /// Key to slot index. First occurrence wins, matching linear search.
    index: HashMap<Rc<str>, u32>,
    /// Memoized `add(key)` children: the same sequence always lands on the
    /// same node, which is what makes shapes canonical and shared.
    transitions: RefCell<HashMap<Rc<str>, Rc<Shape>>>,
}

thread_local! {
    /// The canonical empty layout. Every object starts here conceptually;
    /// every rebuild replays from here.
    static SHAPE_ROOT: Rc<Shape> = Shape::fresh(Vec::new());
    /// Shape id counter. Resetting per thread is fine: shapes never cross
    /// threads (values have a single owner thread).
    static NEXT_SHAPE_ID: Cell<u32> = const { Cell::new(1) };
}

impl Shape {
    fn fresh(keys: Vec<Rc<str>>) -> Rc<Shape> {
        let id = NEXT_SHAPE_ID.with(|next| {
            let id = next.get();
            next.set(id.wrapping_add(1).max(1));
            id
        });
        let mut index = HashMap::with_capacity(keys.len());
        for (slot, key) in keys.iter().enumerate() {
            // First occurrence wins: slot vectors never hold duplicates by
            // construction, but a wholesale host-side replace might, and
            // linear search answers with the first match.
            index.entry(key.clone()).or_insert(slot as u32);
        }
        Rc::new(Shape { id, keys, index, transitions: RefCell::new(HashMap::new()) })
    }

    /// The canonical empty layout.
    pub fn root() -> Rc<Shape> {
        SHAPE_ROOT.with(Rc::clone)
    }

    /// Slot index of `key` in this layout, if present.
    pub fn slot_of(&self, key: &str) -> Option<usize> {
        self.index.get(key).map(|slot| *slot as usize)
    }

    /// The child layout with `key` appended, memoized on this node. Callers
    /// must only transition on genuinely new keys; transitioning on a key
    /// the object already holds would fork a duplicate-key layout, so the
    /// cell checks membership first and rebuilds instead when unsure.
    pub fn add(&self, key: &str) -> Rc<Shape> {
        if let Some(child) = self.transitions.borrow().get(key) {
            return child.clone();
        }
        let mut keys = self.keys.clone();
        keys.push(Rc::from(key));
        let child = Self::fresh(keys);
        self.transitions.borrow_mut().insert(Rc::from(key), child.clone());
        child
    }

    /// Canonical layout for `keys` in order, replayed from the root through
    /// memoized transitions. Deletes and bulk mutations canonicalize through
    /// here: `{a, c}` built directly and `{a, b, c}` minus `b` land on the
    /// same node.
    pub fn rebuild<'a>(keys: impl Iterator<Item = &'a str>) -> Rc<Shape> {
        let mut shape = Self::root();
        for key in keys {
            // Skip repeats so a duplicated slot vector still maps each key
            // to its first slot, exactly like `fresh` does.
            if shape.index.contains_key(key) {
                continue;
            }
            shape = shape.add(key);
        }
        shape
    }
}

/// One bytecode property site's cache: the shape and slot the site last
/// resolved, plus hit/miss counts for observability.
///
/// All interior mutability is `Cell`s, so the VM reads and fills caches
/// with plain loads and stores — no borrow can span a guest call on a
/// re-entrant slow path. `PropCache` is `Send` (every `Cell` holds a `Copy`
/// scalar) but never crosses threads in practice.
#[derive(Debug)]
pub struct PropCache {
    state: Cell<u8>,
    shape: Cell<u32>,
    slot: Cell<u32>,
    /// Distinct shapes seen while monomorphic; past the cap the site goes
    /// megamorphic and stops filling.
    fills: Cell<u8>,
    hits: Cell<u32>,
    misses: Cell<u32>,
}

const CACHE_EMPTY: u8 = 0;
const CACHE_MONO: u8 = 1;
const CACHE_MEGA: u8 = 2;
/// Distinct shapes before a site stops caching and always takes the slow
/// path. Past this point a single entry cannot hit anyway.
const MEGA_AT_FILLS: u8 = 8;

impl PropCache {
    pub const fn empty() -> Self {
        Self {
            state: Cell::new(CACHE_EMPTY),
            shape: Cell::new(0),
            slot: Cell::new(0),
            fills: Cell::new(0),
            hits: Cell::new(0),
            misses: Cell::new(0),
        }
    }

    /// Cached slot for `shape_id`, recording a hit. The caller still
    /// verifies the key at the slot: shapes are caches, not authority.
    pub fn probe(&self, shape_id: u32) -> Option<usize> {
        if self.state.get() == CACHE_MONO && self.shape.get() == shape_id {
            self.hits.set(self.hits.get().wrapping_add(1));
            Some(self.slot.get() as usize)
        } else {
            self.misses.set(self.misses.get().wrapping_add(1));
            None
        }
    }

    /// Record a slow-path resolution. A new shape counts toward
    /// megamorphism; re-resolving the cached shape is free.
    pub fn fill(&self, shape_id: u32, slot: usize) {
        if self.state.get() == CACHE_MEGA {
            return;
        }
        if self.state.get() == CACHE_MONO && self.shape.get() == shape_id {
            self.slot.set(slot as u32);
            return;
        }
        let fills = self.fills.get().saturating_add(1);
        self.fills.set(fills);
        if fills > MEGA_AT_FILLS {
            self.state.set(CACHE_MEGA);
            return;
        }
        self.state.set(CACHE_MONO);
        self.shape.set(shape_id);
        self.slot.set(slot as u32);
    }

    /// Whether this site stopped caching after seeing too many shapes.
    pub fn is_megamorphic(&self) -> bool {
        self.state.get() == CACHE_MEGA
    }

    /// Hit/miss counts since creation, for observability and tests.
    pub fn stats(&self) -> (u32, u32) {
        (self.hits.get(), self.misses.get())
    }
}

impl Clone for PropCache {
    fn clone(&self) -> Self {
        let fresh = Self::empty();
        fresh.state.set(self.state.get());
        fresh.shape.set(self.shape.get());
        fresh.slot.set(self.slot.get());
        fresh.fills.set(self.fills.get());
        fresh.hits.set(self.hits.get());
        fresh.misses.set(self.misses.get());
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::Interpreter;
    use crate::value::Value;

    fn eval(src: &str) -> Value {
        let mut interp = Interpreter::with_builtins();
        interp.eval_source(src).expect("test source must run")
    }

    fn cell_of(value: &Value) -> Rc<crate::value::ObjectCell> {
        match value {
            Value::Object { props } => props.clone(),
            other => panic!("expected object, got {other:?}"),
        }
    }

    #[test]
    fn shared_layout_shared_shape() {
        let pair = eval("[{x: 1, y: 2}, {x: 3, y: 4}]");
        let Value::Array(items) = &pair else { panic!("expected array, got {pair:?}") };
        let items = items.borrow();
        assert_eq!(cell_of(&items[0]).shape_id(), cell_of(&items[1]).shape_id());
    }

    #[test]
    fn add_transition_matches_literal() {
        // The read builds the `[a]` shape so the add follows a transition
        // instead of lazily rebuilding.
        let grown = cell_of(&eval("let o = {a: 1}; o.a; o.b = 2; o;"));
        let literal = cell_of(&eval("({a: 1, b: 2})"));
        assert_eq!(grown.shape_id(), literal.shape_id());
    }

    #[test]
    fn delete_canonicalizes() {
        let shrunk = cell_of(&eval("let o = {a: 1, b: 2, c: 3}; delete o.b; o;"));
        let literal = cell_of(&eval("({a: 1, c: 3})"));
        assert_eq!(shrunk.shape_id(), literal.shape_id());
        // And the survivor still reads correctly by index.
        assert!(matches!(shrunk.own_value("c"), Some(Value::Number(x)) if x == 3.0));
        assert_eq!(shrunk.own_index("b"), None);
    }

    #[test]
    fn attributes_do_not_fork_shapes() {
        let frozen = cell_of(&eval(
            "Object.defineProperty({x: 1}, 'x', {writable: false, enumerable: true, configurable: true});",
        ));
        let plain = cell_of(&eval("({x: 1})"));
        assert_eq!(frozen.shape_id(), plain.shape_id());
    }

    #[test]
    fn bypass_mutation_heals() {
        let props = cell_of(&eval("({p: 1, q: 2})"));
        assert_eq!(props.own_index("p"), Some(0));
        // A host bridge writing through the `Deref` bypasses the shape
        // entirely: the next indexed access must heal, not misread.
        *props.borrow_mut() = vec![("z".to_string(), Value::Number(9.0))];
        assert_eq!(props.own_index("p"), None);
        assert_eq!(props.own_index("z"), Some(0));
        assert!(matches!(props.own_value("z"), Some(Value::Number(x)) if x == 9.0));
    }
}
