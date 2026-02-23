use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{Checkpoint, Idx};

/// Concurrent typed arena with contiguous storage.
///
/// Lock-free allocation via `&self`. Immediate `&T` access after alloc.
/// Contiguous `&[T]` slices. Same [`Idx<T>`] handles and [`Checkpoint<T>`]
/// semantics as [`Arena`](crate::Arena).
///
/// `FastArena<T>` is `Send + Sync` when `T: Send + Sync`.
///
/// # Example
///
/// ```
/// use fast_bump::{FastArena, Idx};
///
/// let arena = FastArena::with_capacity(16);
/// let a: Idx<i32> = arena.alloc(10);
/// let b: Idx<i32> = arena.alloc(20);
///
/// assert_eq!(arena[a], 10);
/// assert_eq!(arena[b], 20);
/// assert_eq!(arena.as_slice(), &[10, 20]);
/// ```
///
/// # Architecture
///
/// Backed by a single contiguous allocation with per-slot readiness flags.
/// Writers claim slots atomically, write values directly in place, then mark
/// the slot as ready. A cooperative `advance_published` protocol makes
/// completed slots visible to readers in order.
///
/// # Comparison with `Arena<T>`
///
/// | Property | `Arena<T>` | `FastArena<T>` |
/// |---|---|---|
/// | `alloc` | `&mut self` | `&self` (concurrent) |
/// | `get` latency | ~1ns | ~1ns |
/// | `&[T]` slices | yes | yes |
/// | `get_mut` | `&mut self` | `&mut self` |
/// | Memory per slot | `size_of::<T>()` | `size_of::<T>()` + 1 byte |
/// | Threading | `Send` | `Send + Sync` |
pub struct FastArena<T> {
    /// Contiguous storage for values. Length = capacity.
    data: *mut T,
    /// Per-slot readiness flags.
    flags: *mut AtomicBool,
    /// Current capacity (number of slots allocated).
    cap: usize,
    /// Next slot to be reserved by `alloc`.
    cursor: AtomicUsize,
    /// Boundary: all slots `< published` are readable.
    published: AtomicUsize,
}

// SAFETY: FastArena owns all data behind raw pointers.
// Access to data[i] is safe when i < published (Acquire fence).
// Writers only write to exclusively reserved slots (cursor.fetch_add).
// T: Send + Sync required for cross-thread value transfer and shared reads.
unsafe impl<T: Send + Sync> Send for FastArena<T> {}
unsafe impl<T: Send + Sync> Sync for FastArena<T> {}

const INITIAL_CAP: usize = 64;

impl<T> FastArena<T> {
    /// Creates a new arena with default initial capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(INITIAL_CAP)
    }

    /// Creates a new arena with the specified capacity.
    ///
    /// The arena will not reallocate until `capacity` items have been
    /// allocated.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = capacity.max(1);
        let (data, flags) = alloc_storage::<T>(cap);
        Self {
            data,
            flags,
            cap,
            cursor: AtomicUsize::new(0),
            published: AtomicUsize::new(0),
        }
    }

    /// Allocates a value, returning its stable index.
    ///
    /// Can be called concurrently from multiple threads (`&self`).
    /// Lock-free, O(1).
    ///
    /// # Panics
    ///
    /// Panics if the arena is full (cursor >= capacity). Call [`grow`]
    /// to expand capacity before this happens.
    pub fn alloc(&self, value: T) -> Idx<T> {
        let slot = self.cursor.fetch_add(1, Ordering::Relaxed);
        assert!(
            slot < self.cap,
            "arena full: slot {slot} >= capacity {}",
            self.cap,
        );

        // SAFETY: slot < cap, and each slot is exclusively owned by the
        // thread that reserved it (unique via fetch_add).
        unsafe {
            self.data.add(slot).write(value);
            (*self.flags.add(slot)).store(true, Ordering::Release);
        }

        self.advance_published(slot);
        Idx::from_raw(slot)
    }

    /// Cooperatively advances `published` past `slot`.
    ///
    /// Same protocol as `SharedArena::advance_published`: each writer
    /// helps advance through all preceding ready slots.
    fn advance_published(&self, slot: usize) {
        loop {
            let p = self.published.load(Ordering::Acquire);
            if p > slot {
                break;
            }
            // SAFETY: p < cap (published never exceeds cursor which is < cap).
            let ready = unsafe { (*self.flags.add(p)).load(Ordering::Acquire) };
            if !ready {
                std::hint::spin_loop();
                continue;
            }
            let _ = self.published.compare_exchange_weak(
                p,
                p + 1,
                Ordering::Release,
                Ordering::Relaxed,
            );
        }
    }

    /// Returns a reference to the value at `idx`.
    ///
    /// Wait-free. Returns `&T` directly.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is out of bounds.
    #[must_use]
    pub fn get(&self, idx: Idx<T>) -> &T {
        let i = idx.into_raw();
        let published = self.published.load(Ordering::Acquire);
        assert!(
            i < published,
            "index out of bounds: index is {i} but published length is {published}",
        );
        // SAFETY: i < published guarantees the slot is written and the
        // Acquire fence synchronizes with the writer's Release store.
        unsafe { &*self.data.add(i) }
    }

    /// Returns a mutable reference to the value at `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is out of bounds.
    #[must_use]
    pub fn get_mut(&mut self, idx: Idx<T>) -> &mut T {
        let i = idx.into_raw();
        let published = *self.published.get_mut();
        assert!(
            i < published,
            "index out of bounds: index is {i} but published length is {published}",
        );
        // SAFETY: &mut self guarantees exclusive access. i < published.
        unsafe { &mut *self.data.add(i) }
    }

    /// Returns a reference to the value at `idx`, or `None` if out of bounds.
    #[must_use]
    pub fn try_get(&self, idx: Idx<T>) -> Option<&T> {
        let i = idx.into_raw();
        if i < self.published.load(Ordering::Acquire) {
            // SAFETY: i < published, same reasoning as get().
            Some(unsafe { &*self.data.add(i) })
        } else {
            None
        }
    }

    /// Returns a mutable reference to the value at `idx`, or `None` if
    /// out of bounds.
    #[must_use]
    pub fn try_get_mut(&mut self, idx: Idx<T>) -> Option<&mut T> {
        let i = idx.into_raw();
        if i < *self.published.get_mut() {
            // SAFETY: &mut self guarantees exclusive access. i < published.
            Some(unsafe { &mut *self.data.add(i) })
        } else {
            None
        }
    }

    /// Returns the number of published (visible) items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.published.load(Ordering::Acquire)
    }

    /// Returns `true` if the arena contains no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the current capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.cap
    }

    /// Returns `true` if `idx` points to a valid item.
    #[must_use]
    pub fn is_valid(&self, idx: Idx<T>) -> bool {
        idx.into_raw() < self.published.load(Ordering::Acquire)
    }

    /// Returns a contiguous slice of all published items.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        let len = self.published.load(Ordering::Acquire);
        if len == 0 {
            return &[];
        }
        // SAFETY: data[0..len] are all written and published. Acquire
        // fence synchronizes with writers.
        unsafe { std::slice::from_raw_parts(self.data, len) }
    }

    /// Returns a mutable slice of all published items.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        let len = *self.published.get_mut();
        if len == 0 {
            return &mut [];
        }
        // SAFETY: &mut self guarantees exclusive access.
        unsafe { std::slice::from_raw_parts_mut(self.data, len) }
    }

    /// Saves the current allocation state.
    #[must_use]
    pub fn checkpoint(&self) -> Checkpoint<T> {
        Checkpoint::from_len(self.published.load(Ordering::Acquire))
    }

    /// Rolls back to a previous checkpoint, dropping all values
    /// allocated after it.
    ///
    /// O(k) where k = number of items dropped.
    ///
    /// # Panics
    ///
    /// Panics if `cp` points beyond the current length.
    pub fn rollback(&mut self, cp: Checkpoint<T>) {
        let current = *self.published.get_mut();
        assert!(
            cp.len() <= current,
            "checkpoint {} beyond current length {current}",
            cp.len(),
        );
        for slot in (cp.len()..current).rev() {
            // SAFETY: slot < current = published, so the value is written.
            // &mut self guarantees exclusive access.
            unsafe {
                self.data.add(slot).drop_in_place();
                (*self.flags.add(slot)).store(false, Ordering::Relaxed);
            }
        }
        *self.published.get_mut() = cp.len();
        *self.cursor.get_mut() = cp.len();
    }

    /// Removes all items, running their destructors.
    ///
    /// Retains allocated storage for reuse.
    pub fn reset(&mut self) {
        let current = *self.published.get_mut();
        for slot in (0..current).rev() {
            // SAFETY: slot < published. &mut self guarantees exclusive access.
            unsafe {
                self.data.add(slot).drop_in_place();
                (*self.flags.add(slot)).store(false, Ordering::Relaxed);
            }
        }
        *self.published.get_mut() = 0;
        *self.cursor.get_mut() = 0;
    }

    /// Doubles the arena capacity.
    ///
    /// Requires `&mut self` — no concurrent readers or writers.
    /// Existing indices remain valid.
    ///
    /// # Panics
    ///
    /// Panics if the new capacity overflows `usize`.
    pub fn grow(&mut self) {
        let new_cap = self.cap.checked_mul(2).expect("capacity overflow");
        self.grow_to(new_cap);
    }

    /// Grows the arena to at least `min_capacity`.
    ///
    /// No-op if current capacity is already sufficient.
    pub fn grow_to(&mut self, min_capacity: usize) {
        if min_capacity <= self.cap {
            return;
        }

        let published = *self.published.get_mut();
        let (new_data, new_flags) = alloc_storage::<T>(min_capacity);

        // SAFETY: copy published items to new storage.
        // &mut self guarantees no concurrent access.
        unsafe {
            std::ptr::copy_nonoverlapping(self.data, new_data, published);
            // Copy flag states
            for i in 0..published {
                let flag_val = (*self.flags.add(i)).load(Ordering::Relaxed);
                (*new_flags.add(i)).store(flag_val, Ordering::Relaxed);
            }
            // Deallocate old storage WITHOUT dropping values (they were moved).
            dealloc_storage(self.data, self.flags, self.cap);
        }

        self.data = new_data;
        self.flags = new_flags;
        self.cap = min_capacity;
    }

    /// Returns an iterator over all published items.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.as_slice().iter()
    }

    /// Returns a mutable iterator over all published items.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.as_mut_slice().iter_mut()
    }

    /// Returns an iterator yielding `(Idx<T>, &T)` pairs.
    #[must_use]
    pub fn iter_indexed(&self) -> crate::IterIndexed<'_, T> {
        crate::IterIndexed::new(self.as_slice().iter().enumerate())
    }

    /// Returns a mutable iterator yielding `(Idx<T>, &mut T)` pairs.
    pub fn iter_indexed_mut(&mut self) -> crate::IterIndexedMut<'_, T> {
        crate::IterIndexedMut::new(self.as_mut_slice().iter_mut().enumerate())
    }

    /// Allocates multiple values from an iterator, returning the index
    /// of the first item.
    ///
    /// Returns `None` if the iterator is empty.
    pub fn alloc_extend(&self, iter: impl IntoIterator<Item = T>) -> Option<Idx<T>> {
        let mut first = None;
        for value in iter {
            let idx = self.alloc(value);
            if first.is_none() {
                first = Some(idx);
            }
        }
        first
    }

    /// Removes all items, returning an iterator that yields them.
    pub fn drain(&mut self) -> std::vec::IntoIter<T> {
        let current = *self.published.get_mut();
        let mut items = Vec::with_capacity(current);
        for slot in 0..current {
            // SAFETY: slot < published. &mut self guarantees exclusive access.
            unsafe {
                items.push(self.data.add(slot).read());
                (*self.flags.add(slot)).store(false, Ordering::Relaxed);
            }
        }
        *self.published.get_mut() = 0;
        *self.cursor.get_mut() = 0;
        items.into_iter()
    }
}

impl<T> Default for FastArena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> std::ops::Index<Idx<T>> for FastArena<T> {
    type Output = T;

    fn index(&self, idx: Idx<T>) -> &T {
        self.get(idx)
    }
}

impl<T> std::ops::IndexMut<Idx<T>> for FastArena<T> {
    fn index_mut(&mut self, idx: Idx<T>) -> &mut T {
        self.get_mut(idx)
    }
}

impl<'a, T> IntoIterator for &'a FastArena<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut FastArena<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T> IntoIterator for FastArena<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(mut self) -> Self::IntoIter {
        self.drain()
    }
}

impl<T> Extend<T> for FastArena<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for value in iter {
            self.alloc(value);
        }
    }
}

impl<T> std::iter::FromIterator<T> for FastArena<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let items: Vec<T> = iter.into_iter().collect();
        let arena = Self::with_capacity(items.len().max(1));
        for value in items {
            arena.alloc(value);
        }
        arena
    }
}

impl<T> Drop for FastArena<T> {
    fn drop(&mut self) {
        let published = *self.published.get_mut();
        // Drop all published values in reverse order.
        for slot in (0..published).rev() {
            // SAFETY: slot < published, values are initialized.
            // &mut self in drop guarantees exclusive access.
            unsafe {
                self.data.add(slot).drop_in_place();
            }
        }
        // SAFETY: dealloc storage without dropping values (already dropped above).
        unsafe {
            dealloc_storage(self.data, self.flags, self.cap);
        }
    }
}

/// Allocates raw storage for `cap` items: a `T` array and `AtomicBool` flags.
///
/// Returns raw pointers to both allocations. Flags are initialized to `false`.
fn alloc_storage<T>(cap: usize) -> (*mut T, *mut AtomicBool) {
    let data_layout = std::alloc::Layout::array::<T>(cap).expect("layout overflow");
    let flags_layout = std::alloc::Layout::array::<AtomicBool>(cap).expect("layout overflow");

    // SAFETY: layouts are valid (non-zero size for cap >= 1).
    let data = unsafe { std::alloc::alloc(data_layout) }.cast::<T>();
    let flags = unsafe { std::alloc::alloc_zeroed(flags_layout) }.cast::<AtomicBool>();

    assert!(!data.is_null(), "allocation failed for data");
    assert!(!flags.is_null(), "allocation failed for flags");

    data.cast::<T>();
    flags.cast::<AtomicBool>();

    (data, flags)
}

/// Deallocates raw storage WITHOUT dropping any values.
///
/// # Safety
///
/// Caller must ensure all live values have been dropped or moved out
/// before calling this.
unsafe fn dealloc_storage<T>(data: *mut T, flags: *mut AtomicBool, cap: usize) {
    let data_layout = std::alloc::Layout::array::<T>(cap).expect("layout overflow");
    let flags_layout = std::alloc::Layout::array::<AtomicBool>(cap).expect("layout overflow");

    unsafe {
        std::alloc::dealloc(data.cast::<u8>(), data_layout);
        std::alloc::dealloc(flags.cast::<u8>(), flags_layout);
    }
}
