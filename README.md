# fast-bump

Fast bump-pointer arena allocator for Rust.

**Contiguous `&[T]`. Lock-free concurrent alloc. Immediate `&T`. Auto `Drop`. Checkpoint/rollback.**

## Why fast-bump?

| Feature | `fast-bump` | `bumpalo` | `typed-arena` | `safe-bump` |
|---------|------------|-----------|---------------|-------------|
| Concurrent alloc (`&self`) | **`FastArena`** | no | no | `SharedArena` |
| Contiguous `&[T]` slices | **yes** | no | no | `Arena` only |
| Immediate `&T` after alloc | **yes** | yes | yes | yes |
| `get` latency | **~1 ns** | ~1 ns | — | ~5-10 ns (`SharedArena`) |
| Memory per slot | **`size_of::<T>()` + 1 byte** | `size_of::<T>()` + padding | `size_of::<T>()` | `size_of::<T>()` + 8 bytes |
| Auto `Drop` | **yes** | no | yes | yes |
| Checkpoint/rollback | **yes** | no | no | yes |
| `get_mut` / `IndexMut` | **yes** | yes | no | `Arena` only |

No other arena on crates.io combines concurrent lock-free allocation
with contiguous `&[T]` slices and immediate `&T` reads.

## Two arena types

### `Arena<T>` — single-thread, zero overhead

Backed by `Vec<T>`. Minimal overhead, cache-friendly linear layout.

```rust
use fast_bump::{Arena, Idx};

let mut arena: Arena<String> = Arena::new();
let a: Idx<String> = arena.alloc(String::from("hello"));
let b: Idx<String> = arena.alloc(String::from("world"));

assert_eq!(arena[a], "hello");
assert_eq!(arena[b], "world");

// Checkpoint and rollback
let cp = arena.checkpoint();
let _tmp = arena.alloc(String::from("temporary"));
assert_eq!(arena.len(), 3);

arena.rollback(cp); // "temporary" is dropped
assert_eq!(arena.len(), 2);
```

### `FastArena<T>` — multi-thread, `Send + Sync`, contiguous

Lock-free concurrent allocation via `&self`. Immediate `&T` reads.
Contiguous `&[T]` slices. Same `Idx<T>` handles, same checkpoint/rollback
semantics.

```rust
use fast_bump::{FastArena, Idx};
use std::sync::Arc;
use std::thread;

let arena = Arc::new(FastArena::<u64>::with_capacity(1000));

let handles: Vec<_> = (0..4).map(|i| {
    let arena = Arc::clone(&arena);
    thread::spawn(move || arena.alloc(i))
}).collect();

let indices: Vec<Idx<u64>> = handles.into_iter().map(|h| h.join().unwrap()).collect();

// All values accessible via &T — no guards, no locks
for idx in &indices {
    let _val: &u64 = arena.get(*idx);
}

// Contiguous slice of all published items
let _slice: &[u64] = arena.as_slice();
```

### Comparison

| Operation | `Arena<T>` | `FastArena<T>` |
|---|---|---|
| `alloc` | `&mut self`, O(1) | `&self`, O(1) lock-free |
| `get` / `try_get` | `&self` → `&T` | `&self` → `&T`, wait-free |
| `get_mut` / `try_get_mut` | `&mut self` → `&mut T` | `&mut self` → `&mut T` |
| `as_slice` | `&self` → `&[T]` | `&self` → `&[T]` |
| `checkpoint` | `&self` | `&self` |
| `rollback` / `reset` | `&mut self` | `&mut self` |
| `iter` / `iter_indexed` | `&self` | `&self` |
| `iter_mut` / `iter_indexed_mut` | `&mut self` | `&mut self` |
| `drain` / `into_iter` | `&mut self` / `self` | `&mut self` / `self` |
| `alloc_extend` | `&mut self` | `&self` |
| `Extend` / `FromIterator` | yes | yes |
| `grow` / `grow_to` | — | `&mut self` |
| Capacity (`with_capacity`, `reserve`, `shrink_to_fit`) | yes | `with_capacity` only |
| **Memory per slot** | **`size_of::<T>()`** | **`size_of::<T>()` + 1 byte** |
| **Cache behavior** | **contiguous** | **contiguous** |
| **Threading** | `Send` | **`Send + Sync`** |

### When to use which

**`Arena<T>`** — default choice for single-thread workloads:
- Backed by `Vec<T>`: one contiguous allocation, cache-friendly sequential access
- `alloc` is a single `Vec::push` — no atomic operations
- `get` is a direct array index — one memory access
- Supports `IndexMut`, `iter_mut`, `Extend`, `FromIterator`

**`FastArena<T>`** — when multiple threads allocate concurrently:
- `alloc(&self)` can be called from any thread — lock-free via atomic cursor
- `get` returns `&T` directly — one pointer offset (~1 ns)
- `as_slice` returns contiguous `&[T]` — cache-friendly iteration
- `grow(&mut self)` expands capacity when exclusive access is available

**The tradeoff:**

| | `Arena<T>` | `FastArena<T>` |
|---|---|---|
| `get` latency | ~1 ns (direct index) | ~1 ns (pointer offset) |
| `alloc` latency | ~5 ns (Vec::push) | ~5-10 ns (atomic + ptr::write) |
| Memory per slot | `size_of::<T>()` | `size_of::<T>()` + 1 byte |
| Empty arena | 0 bytes | capacity × (`size_of::<T>()` + 1) |
| Mutable access | `get_mut`, `IndexMut` | `get_mut`, `IndexMut` (via `&mut self`) |
| Capacity | grows automatically | fixed until `grow(&mut self)` |

The 1-byte overhead comes from a per-slot `AtomicBool` readiness flag used
by the lock-free publication protocol. Values are written directly into
contiguous memory — no indirection, no wrapper types.

If your code is single-threaded, always prefer `Arena<T>` — there is no
reason to pay for synchronization you don't use.

## Design

`Idx<T>` is a stable, `Copy` index valid for the lifetime of the arena
(invalidated by rollback/reset past its allocation point).

`Checkpoint<T>` captures allocation state. Rolling back drops all values
allocated after the checkpoint and reclaims their slots.

Both arena types share the same `Idx<T>` and `Checkpoint<T>` types.

### Architecture of `FastArena`

Backed by a single contiguous allocation (`*mut T`) with a parallel array
of `AtomicBool` readiness flags. Three atomic counters coordinate access:

- **cursor**: writers atomically reserve slots via `fetch_add`
- **flags**: each writer marks its slot as ready after writing
- **published**: cooperative protocol advances the visibility boundary
  through all contiguous ready slots

This gives lock-free `alloc(&self)`, wait-free `get(&self)`, and
contiguous `&[T]` via `as_slice(&self)`.

### Complexity

| Operation | `Arena<T>` | `FastArena<T>` |
|-----------|-----------|--------------------|
| `alloc` | O(1) amortized | O(1) lock-free |
| `get` / `Index` | O(1) | O(1) wait-free |
| `checkpoint` | O(1) | O(1) |
| `rollback` | O(k) | O(k) |
| `reset` | O(n) | O(n) |
| `alloc_extend` | O(n) | O(n) |
| `drain` | O(n) | O(n) |
| `grow` | — | O(n) copy |

k = items dropped (destructors run), n = all items.

### Standard traits

`Arena<T>`: `Index`, `IndexMut`, `IntoIterator`, `Extend`, `FromIterator`, `Default`.

`FastArena<T>`: `Index`, `IndexMut`, `IntoIterator`, `Extend`, `FromIterator`, `Default`.

`Idx<T>`: `Copy`, `Eq`, `Ord`, `Hash`, `Debug`.

`Checkpoint<T>`: `Copy`, `Eq`, `Ord`, `Hash`, `Debug`.

## Limitations

- **Typed**: each arena stores a single type `T`. Use separate arenas for
  different types.
- **Append-only**: individual items cannot be removed. Use `rollback` to
  discard a suffix or `reset` to clear everything.
- **`FastArena` capacity**: does not grow automatically. Call `grow(&mut self)`
  to expand. Panics if `alloc` is called when full.
- **`FastArena` requires `T: Send + Sync`**: values must be safe to share
  across threads.
- **No cross-arena safety**: `Idx<T>` does not carry an arena identifier.
  An index from one arena can be used on another arena of the same type
  (panic on out-of-bounds, wrong data if in-bounds). This is a deliberate
  tradeoff: keeping `Idx` at one machine word minimizes storage overhead and
  eliminates per-access checks on the hot path.

## Verification

`FastArena` unsafe code is verified with [Miri](https://github.com/rust-lang/miri)
on every change — no undefined behavior, no data races.

## References

- Hanson, 1990 — "Fast Allocation and Deallocation of Memory Based on Object Lifetimes"

## License

Apache License 2.0. See [LICENSE](LICENSE).
