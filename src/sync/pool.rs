//! Small scoped worker pool for parallel hashing and file transfers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

/// Run `f` on every item with up to `workers` threads. Results keep input order.
pub fn parallel_map<T, R, F>(items: &[T], workers: usize, f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    if items.len() <= 1 || workers <= 1 {
        return items.iter().map(&f).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    thread::scope(|scope| {
        for _ in 0..workers.min(items.len()) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(i) else { break };
                let result = f(item);
                *slots[i].lock().unwrap_or_else(|p| p.into_inner()) = Some(result);
            });
        }
    });
    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(|p| p.into_inner())
                .expect("every slot is filled before scope ends")
        })
        .collect()
}

/// Like `parallel_map`, but big and small items run in two pools at the same
/// time, so a few big files never block the small ones. Results keep input order.
pub fn split_map<T, R, F>(
    items: &[T],
    is_big: impl Fn(&T) -> bool,
    small_workers: usize,
    big_workers: usize,
    f: F,
) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    let (big, small): (Vec<usize>, Vec<usize>) = (0..items.len()).partition(|&i| is_big(&items[i]));
    let (small_out, big_out) = thread::scope(|scope| {
        let small_run = scope.spawn(|| parallel_map(&small, small_workers, |&i| f(&items[i])));
        let big_out = parallel_map(&big, big_workers, |&i| f(&items[i]));
        (small_run.join().expect("small pool panicked"), big_out)
    });
    let mut slots: Vec<Option<R>> = items.iter().map(|_| None).collect();
    for (i, r) in small
        .into_iter()
        .zip(small_out)
        .chain(big.into_iter().zip(big_out))
    {
        slots[i] = Some(r);
    }
    slots
        .into_iter()
        .map(|r| r.expect("every item has a result"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_input_order() {
        let items: Vec<u64> = (0..100).collect();
        let out = parallel_map(&items, 8, |n| n * 2);
        assert_eq!(out, items.iter().map(|n| n * 2).collect::<Vec<_>>());
    }

    #[test]
    fn split_keeps_input_order() {
        let items: Vec<u64> = (0..100).collect();
        let out = split_map(&items, |n| n % 3 == 0, 8, 2, |n| n * 2);
        assert_eq!(out, items.iter().map(|n| n * 2).collect::<Vec<_>>());
    }

    #[test]
    fn empty_input_is_empty() {
        let out: Vec<u8> = parallel_map(&[] as &[u8], 8, |b| *b);
        assert!(out.is_empty());
    }
}
