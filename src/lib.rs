//! Fast bump-pointer arena allocator.
//!
//! `fast-bump` provides two typed arena allocators. Values are allocated and
//! accessed via stable [`Idx<T>`] indices.
//!
//! # Arena types
//!
//! - [`Arena<T>`] — single-thread, zero overhead, backed by [`Vec<T>`]
//! - [`FastArena<T>`] — concurrent (`Send + Sync`), lock-free allocation,
//!   contiguous `&[T]` slices, immediate `&T` access
//!
//! Both types share the same [`Idx<T>`] and [`Checkpoint<T>`] types, support
//! checkpoint/rollback, and run destructors on rollback/reset/drop.
//!
//! # Key properties
//!
//! - **Auto [`Drop`]**: destructors run on reset, rollback, and arena drop
//! - **Checkpoint/rollback**: save state and discard speculative allocations
//! - **Thread-safe**: [`FastArena<T>`] supports concurrent lock-free allocation
//! - **Contiguous**: both arenas provide `&[T]` slices
//!
//! # Example
//!
//! ```
//! use fast_bump::{Arena, Idx};
//!
//! let mut arena: Arena<String> = Arena::new();
//! let a: Idx<String> = arena.alloc(String::from("hello"));
//! let b: Idx<String> = arena.alloc(String::from("world"));
//!
//! assert_eq!(arena[a], "hello");
//! assert_eq!(arena[b], "world");
//!
//! let cp = arena.checkpoint();
//! let _tmp = arena.alloc(String::from("temporary"));
//! arena.rollback(cp); // "temporary" is dropped
//! assert_eq!(arena.len(), 2);
//! ```

#![deny(missing_docs)]

mod arena;
mod checkpoint;
mod fast_arena;
mod idx;
mod iter;

pub use arena::Arena;
pub use checkpoint::Checkpoint;
pub use fast_arena::FastArena;
pub use idx::Idx;
pub use iter::{IterIndexed, IterIndexedMut};

#[cfg(test)]
mod tests;
