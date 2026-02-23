use std::sync::Arc;
use std::thread;

use crate::{Checkpoint, FastArena, Idx};

use super::Tracked;

#[test]
fn alloc_and_get() {
    let arena = FastArena::with_capacity(16);
    let a = arena.alloc(10);
    let b = arena.alloc(20);
    let c = arena.alloc(30);

    assert_eq!(arena[a], 10);
    assert_eq!(arena[b], 20);
    assert_eq!(arena[c], 30);
}

#[test]
fn len_and_is_empty() {
    let arena = FastArena::with_capacity(16);
    assert!(arena.is_empty());
    assert_eq!(arena.len(), 0);

    arena.alloc(1);
    assert!(!arena.is_empty());
    assert_eq!(arena.len(), 1);

    arena.alloc(2);
    assert_eq!(arena.len(), 2);
}

#[test]
fn as_slice() {
    let arena = FastArena::with_capacity(16);
    arena.alloc(10);
    arena.alloc(20);
    arena.alloc(30);

    assert_eq!(arena.as_slice(), &[10, 20, 30]);
}

#[test]
fn as_slice_empty() {
    let arena = FastArena::<i32>::with_capacity(16);
    assert_eq!(arena.as_slice(), &[] as &[i32]);
}

#[test]
fn get_mut() {
    let mut arena = FastArena::with_capacity(16);
    let a = arena.alloc(10);

    *arena.get_mut(a) = 42;
    assert_eq!(arena[a], 42);
}

#[test]
fn try_get() {
    let arena = FastArena::with_capacity(16);
    let a = arena.alloc(10);

    assert_eq!(arena.try_get(a), Some(&10));
    assert_eq!(arena.try_get(Idx::from_raw(99)), None);
}

#[test]
fn try_get_mut() {
    let mut arena = FastArena::with_capacity(16);
    let a = arena.alloc(10);

    assert_eq!(arena.try_get_mut(Idx::from_raw(99)), None);
    *arena.try_get_mut(a).unwrap() = 42;
    assert_eq!(arena[a], 42);
}

#[test]
fn is_valid() {
    let arena = FastArena::with_capacity(16);
    let a = arena.alloc(10);

    assert!(arena.is_valid(a));
    assert!(!arena.is_valid(Idx::from_raw(99)));
}

#[test]
fn checkpoint_and_rollback() {
    let mut arena = FastArena::with_capacity(16);
    let a = arena.alloc(String::from("keep"));
    let cp = arena.checkpoint();
    let _b = arena.alloc(String::from("discard"));
    assert_eq!(arena.len(), 2);

    arena.rollback(cp);
    assert_eq!(arena.len(), 1);
    assert_eq!(arena[a], "keep");
}

#[test]
fn rollback_runs_destructors() {
    use std::cell::Cell;
    use std::rc::Rc;

    let drops = Rc::new(Cell::new(0u32));
    let mut arena = FastArena::with_capacity(16);
    arena.alloc(Tracked(Rc::clone(&drops)));
    let cp = arena.checkpoint();
    arena.alloc(Tracked(Rc::clone(&drops)));
    arena.alloc(Tracked(Rc::clone(&drops)));
    assert_eq!(drops.get(), 0);

    arena.rollback(cp);
    assert_eq!(drops.get(), 2);
}

#[test]
fn reset() {
    use std::cell::Cell;
    use std::rc::Rc;

    let drops = Rc::new(Cell::new(0u32));
    let mut arena = FastArena::with_capacity(16);
    arena.alloc(Tracked(Rc::clone(&drops)));
    arena.alloc(Tracked(Rc::clone(&drops)));
    arena.alloc(Tracked(Rc::clone(&drops)));

    arena.reset();
    assert_eq!(arena.len(), 0);
    assert_eq!(drops.get(), 3);
}

#[test]
fn drop_runs_destructors() {
    use std::cell::Cell;
    use std::rc::Rc;

    let drops = Rc::new(Cell::new(0u32));
    {
        let arena = FastArena::with_capacity(16);
        arena.alloc(Tracked(Rc::clone(&drops)));
        arena.alloc(Tracked(Rc::clone(&drops)));
    }
    assert_eq!(drops.get(), 2);
}

#[test]
fn grow() {
    let mut arena = FastArena::with_capacity(2);
    let a = arena.alloc(10);
    let b = arena.alloc(20);
    assert_eq!(arena.capacity(), 2);

    arena.grow();
    assert_eq!(arena.capacity(), 4);
    assert_eq!(arena[a], 10);
    assert_eq!(arena[b], 20);

    let c = arena.alloc(30);
    assert_eq!(arena[c], 30);
    assert_eq!(arena.as_slice(), &[10, 20, 30]);
}

#[test]
fn grow_to() {
    let mut arena = FastArena::with_capacity(2);
    arena.alloc(1);
    arena.alloc(2);

    arena.grow_to(100);
    assert_eq!(arena.capacity(), 100);
    assert_eq!(arena.as_slice(), &[1, 2]);
}

#[test]
fn grow_to_noop_if_sufficient() {
    let mut arena = FastArena::with_capacity(100);
    arena.alloc(1);
    arena.grow_to(50);
    assert_eq!(arena.capacity(), 100);
}

#[test]
fn concurrent_alloc_4_threads() {
    let arena = Arc::new(FastArena::with_capacity(4000));

    let all_indices: Vec<(Idx<i32>, i32)> = (0..4)
        .map(|t| {
            let arena = Arc::clone(&arena);
            thread::spawn(move || {
                let mut indices = Vec::with_capacity(1000);
                for i in 0..1000 {
                    let idx = arena.alloc(t * 1000 + i);
                    indices.push((idx, t * 1000 + i));
                }
                indices
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();

    assert_eq!(arena.len(), 4000);

    for (idx, expected) in &all_indices {
        assert_eq!(arena[*idx], *expected);
    }
}

#[test]
fn concurrent_alloc_and_read() {
    let arena = Arc::new(FastArena::with_capacity(1000));

    // Allocate some initial values
    for i in 0..100 {
        arena.alloc(i);
    }

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let arena = Arc::clone(&arena);
            thread::spawn(move || {
                // Read existing values while others allocate
                for i in 0..100 {
                    let val = arena.get(Idx::from_raw(i));
                    assert_eq!(*val, i32::try_from(i).unwrap());
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn as_slice_contiguous() {
    let arena = FastArena::with_capacity(16);
    arena.alloc(1);
    arena.alloc(2);
    arena.alloc(3);
    arena.alloc(4);
    arena.alloc(5);

    let slice = arena.as_slice();
    assert_eq!(slice, &[1, 2, 3, 4, 5]);

    // Verify contiguity: addresses are sequential
    for i in 0..slice.len() - 1 {
        let addr_a = &raw const slice[i];
        let addr_b = &raw const slice[i + 1];
        assert_eq!(unsafe { addr_a.add(1) }, addr_b);
    }
}

#[test]
fn alloc_extend() {
    let arena = FastArena::with_capacity(16);
    let first = arena.alloc_extend(vec![10, 20, 30]);

    assert_eq!(first, Some(Idx::from_raw(0)));
    assert_eq!(arena.as_slice(), &[10, 20, 30]);
}

#[test]
fn alloc_extend_empty() {
    let arena = FastArena::<i32>::with_capacity(16);
    let first = arena.alloc_extend(Vec::new());
    assert_eq!(first, None);
}

#[test]
fn drain() {
    let mut arena = FastArena::with_capacity(16);
    arena.alloc(10);
    arena.alloc(20);
    arena.alloc(30);

    let items: Vec<i32> = arena.drain().collect();
    assert_eq!(items, vec![10, 20, 30]);
    assert_eq!(arena.len(), 0);
}

#[test]
fn into_iterator() {
    let arena = FastArena::with_capacity(16);
    arena.alloc(10);
    arena.alloc(20);
    arena.alloc(30);

    let items: Vec<i32> = arena.into_iter().collect();
    assert_eq!(items, vec![10, 20, 30]);
}

#[test]
fn iter_ref() {
    let arena = FastArena::with_capacity(16);
    arena.alloc(10);
    arena.alloc(20);
    arena.alloc(30);

    let items: Vec<&i32> = arena.iter().collect();
    assert_eq!(items, vec![&10, &20, &30]);
}

#[test]
fn iter_mut() {
    let mut arena = FastArena::with_capacity(16);
    arena.alloc(10);
    arena.alloc(20);
    arena.alloc(30);

    for val in &mut arena {
        *val *= 2;
    }
    assert_eq!(arena.as_slice(), &[20, 40, 60]);
}

#[test]
fn iter_indexed() {
    let arena = FastArena::with_capacity(16);
    let a = arena.alloc(10);
    let b = arena.alloc(20);

    let items: Vec<_> = arena.iter_indexed().collect();
    assert_eq!(items, vec![(a, &10), (b, &20)]);
}

#[test]
fn extend_trait() {
    let mut arena = FastArena::with_capacity(16);
    arena.extend(vec![10, 20, 30]);
    assert_eq!(arena.as_slice(), &[10, 20, 30]);
}

#[test]
fn from_iterator() {
    let arena: FastArena<i32> = vec![10, 20, 30].into_iter().collect();
    assert_eq!(arena.as_slice(), &[10, 20, 30]);
}

#[test]
fn index_trait() {
    let arena = FastArena::with_capacity(16);
    let a = arena.alloc(42);
    assert_eq!(arena[a], 42);
}

#[test]
fn index_mut_trait() {
    let mut arena = FastArena::with_capacity(16);
    let a = arena.alloc(42);
    arena[a] = 99;
    assert_eq!(arena[a], 99);
}

#[test]
#[should_panic(expected = "arena full")]
fn panics_when_full() {
    let arena = FastArena::with_capacity(2);
    arena.alloc(1);
    arena.alloc(2);
    arena.alloc(3); // panic
}

#[test]
#[should_panic(expected = "index out of bounds")]
fn panics_on_invalid_get() {
    let arena = FastArena::<i32>::with_capacity(16);
    let _ = arena.get(Idx::from_raw(0));
}

#[test]
#[should_panic(expected = "checkpoint")]
fn panics_on_invalid_rollback() {
    let mut arena = FastArena::with_capacity(16);
    arena.alloc(1);
    let invalid_cp = Checkpoint::from_len(10);
    arena.rollback(invalid_cp);
}

#[test]
fn reuse_after_reset() {
    let mut arena = FastArena::with_capacity(16);
    arena.alloc(1);
    arena.alloc(2);
    arena.reset();

    let a = arena.alloc(10);
    assert_eq!(arena[a], 10);
    assert_eq!(arena.len(), 1);
}

#[test]
fn reuse_after_rollback() {
    let mut arena = FastArena::with_capacity(16);
    let cp = arena.checkpoint();
    arena.alloc(1);
    arena.alloc(2);
    arena.rollback(cp);

    let a = arena.alloc(10);
    assert_eq!(arena[a], 10);
    assert_eq!(arena.len(), 1);
}

#[test]
fn default_creates_empty() {
    let arena = FastArena::<i32>::default();
    assert!(arena.is_empty());
    assert_eq!(arena.capacity(), 64);
}

#[test]
fn capacity() {
    let arena = FastArena::<i32>::with_capacity(128);
    assert_eq!(arena.capacity(), 128);
}
