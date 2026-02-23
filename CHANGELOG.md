# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-02-25

Initial release. Forked from [safe-bump](https://github.com/YuriyKrasilnikov/safe-bump).

### Added
- `FastArena<T>` — concurrent (`Send + Sync`) typed arena with contiguous
  storage, lock-free `alloc(&self)`, immediate `&T` access, and `&[T]` slices.
- Backed by a single contiguous allocation with per-slot `AtomicBool`
  readiness flags (1 byte overhead per slot).
- Lock-free publication protocol: atomic cursor + cooperative
  `advance_published` for ordered visibility.
- `alloc`, `alloc_extend` — concurrent allocation.
- `get`, `get_mut`, `try_get`, `try_get_mut`, `is_valid` — index access.
- `as_slice`, `as_mut_slice` — contiguous slice access.
- `checkpoint`, `rollback`, `reset` — speculative allocation support.
- `iter`, `iter_mut`, `iter_indexed`, `iter_indexed_mut` — iteration.
- `drain`, `into_iter` — consuming iteration.
- `grow`, `grow_to` — capacity expansion (`&mut self`).
- `Index`/`IndexMut`, `Extend`, `FromIterator`, `IntoIterator` trait impls.
- `Arena<T>` — single-thread arena from safe-bump (unchanged).
- Miri verification for all unsafe code.
