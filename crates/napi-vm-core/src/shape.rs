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
//! same cached add sequence from the root reaches the same node no matter
//! which interpreter or realm built it; deletes rebuild by replaying the
//! surviving keys from the root, which canonicalizes them the same way.
//! A bounded owner-local cache evicts only unused branches; live layouts keep
//! their shared identity. New layouts still work when the cache is full.
//! Shape ids come from a thread-local counter and are never reused, so an
//! inline-cache guard comparing them cannot alias across layouts.
//!
//! [`ObjectCell`]: crate::value::ObjectCell
//! [`ObjectMeta`]: crate::value::ObjectMeta

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

// Bound the complete owner-local transition tree, not each node independently.
// A per-node limit still permits exponential retention across key prefixes.
const MAX_CACHED_TRANSITIONS: usize = 4096;
const PRUNE_MISS_INTERVAL: usize = 1024;

#[derive(Debug, Default)]
struct TransitionBudget {
    entries: Cell<usize>,
    misses_until_prune: Cell<usize>,
    root: RefCell<Weak<Shape>>,
}

impl TransitionBudget {
    fn can_cache(&self) -> bool {
        if self.entries.get() < MAX_CACHED_TRANSITIONS {
            return true;
        }
        let remaining = self.misses_until_prune.get();
        if remaining == 0 {
            // Upgrade outside the RefCell borrow: pruning can drop whole trees.
            let root = self.root.borrow().upgrade();
            if let Some(root) = root {
                root.prune_unused_transitions();
            }
            self.misses_until_prune.set(PRUNE_MISS_INTERVAL);
        } else {
            self.misses_until_prune.set(remaining - 1);
        }
        self.entries.get() < MAX_CACHED_TRANSITIONS
    }
}

/// Canonical descriptor of one object key layout: insertion-ordered keys
/// plus a key-to-slot index, with memoized add transitions to children.
// Group the hot inline-cache ID and slot index before the cold layout keys
// and transition policy. Rc's counters precede this header in its allocation.
#[repr(C)]
#[derive(Debug)]
pub(crate) struct Shape {
    /// Unique within this thread; never reused. Inline caches guard on it.
    pub id: u32,
    /// Key to slot index. First occurrence wins, matching linear search.
    index: HashMap<Rc<str>, u32>,
    /// Own keys in insertion order, parallel to the object's slot vector.
    keys: Vec<Rc<str>>,
    /// Memoized `add(key)` children: the same sequence always lands on the
    /// same node, which is what makes shapes canonical and shared.
    transitions: RefCell<HashMap<Rc<str>, Rc<Shape>>>,
    budget: Rc<TransitionBudget>,
}

thread_local! {
    /// The canonical empty layout. Every object starts here conceptually;
    /// every rebuild replays from here.
    static SHAPE_ROOT: RefCell<Rc<Shape>> = RefCell::new(Shape::fresh(Vec::new()));
    /// Shape id counter. Resetting per thread is fine: shapes never cross
    /// threads (values have a single owner thread).
    static NEXT_SHAPE_ID: Cell<u32> = const { Cell::new(1) };
}

#[cfg(feature = "napi")]
pub(crate) struct ShapeContext {
    root: Rc<Shape>,
    next: u32,
}
#[cfg(feature = "napi")]
impl Default for ShapeContext {
    fn default() -> Self {
        // Constructing a detached owner must not consume the active owner's
        // shape IDs. Its counter starts independently when it is leased.
        let budget = Rc::new(TransitionBudget::default());
        let root = Rc::new(Shape {
            id: 1,
            keys: Vec::new(),
            index: HashMap::new(),
            transitions: RefCell::new(HashMap::new()),
            budget: budget.clone(),
        });
        *budget.root.borrow_mut() = Rc::downgrade(&root);
        Self { root, next: 2 }
    }
}
#[cfg(feature = "napi")]
impl ShapeContext {
    pub(crate) fn swap_active(&mut self) {
        SHAPE_ROOT.with(|root| std::mem::swap(&mut *root.borrow_mut(), &mut self.root));
        NEXT_SHAPE_ID.with(|next| {
            let previous = next.replace(self.next);
            self.next = previous;
        });
    }
}

impl Shape {
    fn fresh(keys: Vec<Rc<str>>) -> Rc<Shape> {
        let budget = Rc::new(TransitionBudget::default());
        let root = Self::fresh_in(keys, budget.clone());
        // Only the canonical root needs a back-reference. Detached dictionary
        // shapes must remain weak-free so Rc::get_mut can extend them in place.
        if root.keys.is_empty() {
            *budget.root.borrow_mut() = Rc::downgrade(&root);
        }
        root
    }

    fn fresh_in(keys: Vec<Rc<str>>, budget: Rc<TransitionBudget>) -> Rc<Shape> {
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
        Rc::new(Shape {
            id,
            keys,
            index,
            transitions: RefCell::new(HashMap::new()),
            budget,
        })
    }

    /// The canonical empty layout.
    pub fn root() -> Rc<Shape> {
        SHAPE_ROOT.with(|root| root.borrow().clone())
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
        let key: Rc<str> = Rc::from(key);
        keys.push(key.clone());
        let cache = self.keys.len() < 128 && self.budget.can_cache();
        let child = Self::fresh_in(keys, self.budget.clone());
        // Dictionary-sized layouts must not retain every prefix's full key
        // vector and index. Keep canonical transitions for small objects.
        if cache {
            self.transitions.borrow_mut().insert(key, child.clone());
            self.budget.entries.set(self.budget.entries.get() + 1);
        }
        child
    }

    /// Remove cache-only leaves bottom-up. An ancestor of a live layout stays
    /// canonical even when only its descendant has an external Rc owner.
    /// Cached paths have at most 128 keys, bounding recursion depth.
    fn prune_unused_transitions(&self) {
        let mut transitions = self.transitions.borrow_mut();
        transitions.retain(|_, child| {
            child.prune_unused_transitions();
            let unused = Rc::strong_count(child) == 1 && child.transitions.borrow().is_empty();
            if unused {
                self.budget.entries.set(self.budget.entries.get() - 1);
            }
            !unused
        });
        // Live prefixes must not retain large empty hash tables after pruning.
        // Keep a small reserve to avoid reallocating for ordinary hot layouts.
        let reserve = transitions.len().max(64);
        if transitions.capacity() > reserve * 4 {
            transitions.shrink_to(reserve);
        }
    }

    /// Dictionary-sized layouts are detached from the canonical transition
    /// tree. Extend a uniquely owned one without cloning its entire index.
    /// Mint a new id so every inline cache still observes the layout change.
    pub fn append(shape: &mut Rc<Shape>, key: &str) {
        if shape.keys.len() >= 128
            && let Some(layout) = Rc::get_mut(shape)
        {
            layout.id = NEXT_SHAPE_ID.with(|next| {
                let id = next.get();
                next.set(id.wrapping_add(1).max(1));
                id
            });
            let key: Rc<str> = Rc::from(key);
            let slot = layout.keys.len() as u32;
            layout.index.entry(key.clone()).or_insert(slot);
            layout.keys.push(key);
            debug_assert!(layout.transitions.get_mut().is_empty());
            return;
        }
        *shape = shape.add(key);
    }

    /// Canonical layout for `keys` in order, replayed from the root through
    /// memoized transitions. Deletes and bulk mutations canonicalize through
    /// Shapes minted on this thread: the id counter less the root. Deltas
    /// across a workload measure layout churn; sharing keeps it far below
    /// the object count.
    pub fn created_count() -> u32 {
        NEXT_SHAPE_ID.with(|next| next.get().saturating_sub(1))
    }

    /// Canonical layout for `keys` in order, replayed from the root through
    /// memoized transitions. Deletes and bulk mutations canonicalize through
    /// here: `{a, c}` built directly and `{a, b, c}` minus `b` land on the
    /// same node.
    pub fn rebuild<'a>(keys: impl Iterator<Item = &'a str>) -> Rc<Shape> {
        let keys: Vec<_> = keys.collect();
        if keys.len() > 128 {
            return Self::fresh(keys.into_iter().map(Rc::from).collect());
        }
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

impl Drop for Shape {
    fn drop(&mut self) {
        // Detached/live layouts can outlive their root or a pruned prefix.
        // Account their own outgoing edges before recursive field destruction.
        self.budget
            .entries
            .set(self.budget.entries.get() - self.transitions.get_mut().len());
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

    /// Cached slot for `shape_id`, recording a hit. `None` (no layout
    /// cached yet) always misses fast. The caller still verifies the key
    /// at the slot: shapes are caches, not authority.
    pub fn probe(&self, shape_id: Option<u32>) -> Option<usize> {
        if let Some(id) = shape_id
            && self.state.get() == CACHE_MONO
            && self.shape.get() == id
        {
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

#[doc(hidden)]
pub fn created_count() -> u32 {
    Shape::created_count()
}

#[doc(hidden)]
pub fn root_identity() -> usize {
    Rc::as_ptr(&Shape::root()) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::Interpreter;
    use crate::value::Value;

    struct PinnedValue {
        value: Value,
        _pin: crate::heap::RootPin,
    }
    impl std::ops::Deref for PinnedValue {
        type Target = Value;
        fn deref(&self) -> &Value {
            &self.value
        }
    }
    struct PinnedCell {
        cell: Rc<crate::value::ObjectCell>,
        _pin: crate::heap::RootPin,
    }
    impl std::ops::Deref for PinnedCell {
        type Target = crate::value::ObjectCell;
        fn deref(&self) -> &Self::Target {
            &self.cell
        }
    }
    fn eval(src: &str) -> PinnedValue {
        let mut interp = Interpreter::with_builtins();
        let value = interp.eval_source(src).expect("test source must run");
        let pin = crate::heap::RootPin::new(value.clone());
        drop(interp);
        // Host-owned results (including earlier cells in the same test)
        // stay pinned while unreachable builtin cycles are reclaimed.
        assert_eq!(crate::heap::collect().skipped, None);
        PinnedValue { value, _pin: pin }
    }

    fn cell_of(value: &Value) -> PinnedCell {
        match value {
            Value::Object { props } => PinnedCell {
                cell: props.clone(),
                _pin: crate::heap::RootPin::new(value.clone()),
            },
            other => panic!("expected object, got {other:?}"),
        }
    }

    /// Read `key` twice so the cell builds its layout, then return the id.
    /// Shapes build lazily; comparing unbuilt cells would pass vacuously.
    fn built_id(cell: &crate::value::ObjectCell, key: &str) -> u32 {
        cell.own_index(key);
        cell.own_index(key);
        cell.shape_id().expect("two reads build the layout")
    }

    #[test]
    fn cache_budget_preserves_live_descendants_and_evicts_unused_layouts() {
        let root = Shape::fresh(Vec::new());
        // Keep only the leaf externally: its prefix must survive pruning too.
        let live = root.add("hot").add("value");
        let old = root.add("evict");
        let old_id = old.id;
        let weak = Rc::downgrade(&old);
        let cache = PropCache::empty();
        cache.fill(old_id, 0);
        drop(old);
        for i in 0..MAX_CACHED_TRANSITIONS * 4 {
            root.add(&format!("unique_{i}"));
            assert!(root.budget.entries.get() <= MAX_CACHED_TRANSITIONS);
        }
        assert!(
            weak.upgrade().is_none(),
            "unused cached layout was retained"
        );
        assert!(Rc::ptr_eq(&live, &root.add("hot").add("value")));
        let replacement = root.add("evict");
        assert_ne!(replacement.id, old_id);
        assert_eq!(cache.probe(Some(replacement.id)), None);
        assert_eq!(replacement.slot_of("evict"), Some(0));
    }

    #[test]
    fn budget_bounds_the_entire_tree_even_when_every_layout_is_live() {
        let root = Shape::fresh(Vec::new());
        let mut layouts = Vec::new();
        for i in 0..MAX_CACHED_TRANSITIONS * 2 {
            let prefix = root.add(&format!("prefix_{}", i % 32));
            let key = format!("key_{i}");
            let layout = prefix.add(&key);
            assert_eq!(layout.slot_of(&key), Some(1));
            layouts.push(layout);
            assert!(root.budget.entries.get() <= MAX_CACHED_TRANSITIONS);
        }
        assert_eq!(root.budget.entries.get(), MAX_CACHED_TRANSITIONS);
        drop(layouts);
        root.prune_unused_transitions();
        assert_eq!(root.budget.entries.get(), 0);
        assert!(root.transitions.borrow().is_empty());
    }

    #[test]
    fn pruning_releases_bucket_storage_on_a_live_prefix() {
        let root = Shape::fresh(Vec::new());
        let prefix = root.add("prefix");
        for i in 0..MAX_CACHED_TRANSITIONS - 1 {
            prefix.add(&format!("temporary_{i}"));
        }
        let capacity = prefix.transitions.borrow().capacity();
        root.prune_unused_transitions();
        assert!(prefix.transitions.borrow().is_empty());
        assert!(prefix.transitions.borrow().capacity() < capacity / 2);
        assert!(Rc::ptr_eq(&prefix, &root.add("prefix")));
    }

    #[test]
    fn detached_layout_accounting_survives_root_destruction() {
        let root = Shape::fresh(Vec::new());
        let budget = root.budget.clone();
        let parent = root.add("parent");
        let leaf = parent.add("leaf");
        assert_eq!(budget.entries.get(), 2);
        drop(root);
        assert_eq!(budget.entries.get(), 1);
        assert!(budget.root.borrow().upgrade().is_none());
        drop(parent);
        assert_eq!(budget.entries.get(), 0);
        assert_eq!(leaf.slot_of("leaf"), Some(1));
        drop(leaf);
        assert_eq!(budget.entries.get(), 0);
    }

    #[cfg(feature = "napi")]
    #[test]
    fn owner_context_keeps_its_budget_and_counter_isolated() {
        let active = Shape::root();
        let before = Shape::created_count();
        let mut owner = ShapeContext::default();
        assert_eq!(Shape::created_count(), before);
        assert!(!Rc::ptr_eq(&active.budget, &owner.root.budget));
        owner.swap_active();
        let root = Shape::root();
        let layout = root.add("owner_key");
        assert_eq!(root.budget.entries.get(), 1);
        owner.swap_active();
        assert!(Rc::ptr_eq(&Shape::root(), &active));
        assert_eq!(Shape::created_count(), before);
        assert_eq!(owner.root.budget.entries.get(), 1);
        assert_eq!(layout.slot_of("owner_key"), Some(0));
    }

    #[test]
    fn shared_layout_shared_shape() {
        let pair = eval("[{x: 1, y: 2}, {x: 3, y: 4}]");
        let Value::Array(items) = &*pair else {
            panic!("expected array, got {:?}", pair.value)
        };
        let items = items.borrow();
        assert_eq!(
            built_id(&cell_of(&items[0]), "x"),
            built_id(&cell_of(&items[1]), "x")
        );
    }

    #[test]
    fn single_read_builds_nothing() {
        let props = cell_of(&eval("({p: 1, q: 2})"));
        assert_eq!(props.shape_id(), None);
        assert_eq!(props.own_index("p"), Some(0));
        assert_eq!(
            props.shape_id(),
            None,
            "one read must not allocate a layout"
        );
        assert_eq!(props.own_index("p"), Some(0));
        assert!(props.shape_id().is_some(), "the repeat read builds it");
    }

    #[test]
    fn add_transition_matches_literal() {
        // Two reads build the `[a]` shape so the add follows a transition
        // instead of lazily rebuilding.
        let value = eval("({a: 1})");
        let grown = cell_of(&value);
        built_id(&grown, "a");
        value.set_prop("b".into(), Value::Number(2.0)).unwrap();
        let literal = cell_of(&eval("({a: 1, b: 2})"));
        assert_eq!(grown.shape_id(), Some(built_id(&literal, "a")));
    }

    #[test]
    fn delete_canonicalizes() {
        let shrunk = cell_of(&eval("let o = {a: 1, b: 2, c: 3}; delete o.b; o;"));
        let literal = cell_of(&eval("({a: 1, c: 3})"));
        assert_eq!(built_id(&shrunk, "a"), built_id(&literal, "a"));
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
        assert_eq!(built_id(&frozen, "x"), built_id(&plain, "x"));
    }

    #[test]
    fn bypass_mutation_heals() {
        let props = cell_of(&eval("({p: 1, q: 2})"));
        assert_eq!(props.own_index("p"), Some(0));
        assert_eq!(props.own_index("p"), Some(0));
        assert!(props.shape_id().is_some());
        // A host bridge writing through the `Deref` bypasses the shape
        // entirely: the next indexed access must heal, not misread.
        *props.borrow_mut() = vec![("z".to_string(), Value::Number(9.0))];
        assert_eq!(props.own_index("p"), None);
        assert_eq!(props.own_index("z"), Some(0));
        assert!(matches!(props.own_value("z"), Some(Value::Number(x)) if x == 9.0));
    }

    #[test]
    fn unique_dictionary_growth_preserves_storage_and_invalidates_caches() {
        let keys: Vec<String> = (0..129).map(|index| format!("key{index}")).collect();
        let mut shape = Shape::rebuild(keys.iter().map(String::as_str));
        let address = Rc::as_ptr(&shape);
        let previous_id = shape.id;
        let cache = PropCache::empty();
        cache.fill(previous_id, 1);
        for index in 129..4096 {
            Shape::append(&mut shape, &format!("key{index}"));
        }
        assert_eq!(Rc::as_ptr(&shape), address);
        assert_ne!(shape.id, previous_id);
        assert_eq!(cache.probe(Some(shape.id)), None);
        assert_eq!(shape.slot_of("key1"), Some(1));
        assert_eq!(shape.slot_of("key4095"), Some(4095));
    }

    #[test]
    fn shared_dictionary_growth_detaches_without_changing_the_other_layout() {
        let keys: Vec<String> = (0..129).map(|index| format!("key{index}")).collect();
        let mut shape = Shape::rebuild(keys.iter().map(String::as_str));
        let other = shape.clone();
        let previous_id = other.id;
        Shape::append(&mut shape, "newKey");
        assert_ne!(shape.id, previous_id);
        assert_eq!(other.id, previous_id);
        assert_eq!(other.slot_of("newKey"), None);
        assert_eq!(shape.slot_of("newKey"), Some(129));
    }
}
