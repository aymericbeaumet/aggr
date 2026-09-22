//! Ordered CPU work with bounded scoped workers and a cheap path for small builds.
//!
//! Workers claim the next unclaimed input through a shared cursor and write into preallocated
//! slots, so a few slow inputs never leave the other threads idle while the output keeps input
//! order. `AGGR_BUILD_WORKERS` pins the thread count; tests pin it per thread instead.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use anyhow::{Result, anyhow};

/// Scoped worker threads never exceed this, whatever the machine or the override says.
const MAX_WORKERS: usize = 8;
/// Below this many inputs, spawning threads costs more than it saves.
const PARALLEL_THRESHOLD: usize = 8;

pub(crate) fn map<T, U, F>(inputs: &[T], transform: F) -> Result<Vec<U>>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> Result<U> + Sync,
{
    map_with_workers(inputs, transform, workers())
}

/// Worker count for this build, between one and [`MAX_WORKERS`]: a test pin first, then the
/// `AGGR_BUILD_WORKERS` override when it is a positive integer, otherwise available parallelism.
pub(crate) fn workers() -> usize {
    #[cfg(test)]
    if let Some(pinned) = pinned_workers() {
        return pinned.clamp(1, MAX_WORKERS);
    }
    let requested = std::env::var("AGGR_BUILD_WORKERS")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| {
            let parsed = parse_workers(&value);
            if parsed.is_none() {
                log::warn!("ignoring AGGR_BUILD_WORKERS={value:?}: expected a positive integer");
            }
            parsed
        });
    requested
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from))
        .clamp(1, MAX_WORKERS)
}

/// Inputs per batch for a phase that gathers in parallel but must bound what it holds in memory:
/// about two per worker, never below the size at which [`map`] stays single-threaded.
pub(super) fn window() -> usize {
    (2 * workers()).max(PARALLEL_THRESHOLD)
}

/// `AGGR_BUILD_WORKERS` as a worker count; zero or anything but an integer keeps the default.
fn parse_workers(value: &str) -> Option<usize> {
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|workers| *workers > 0)
}

fn map_with_workers<T, U, F>(inputs: &[T], transform: F, workers: usize) -> Result<Vec<U>>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> Result<U> + Sync,
{
    let workers = workers.clamp(1, MAX_WORKERS).min(inputs.len().max(1));
    if workers == 1 || inputs.len() < PARALLEL_THRESHOLD {
        return std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            inputs.iter().map(transform).collect()
        }))
        .unwrap_or_else(|_| Err(panicked()));
    }
    let slots: Vec<Mutex<Option<Result<U>>>> = inputs.iter().map(|_| Mutex::new(None)).collect();
    let cursor = AtomicUsize::new(0);
    let worker = || {
        loop {
            // Claim the next unclaimed input; one slow input never idles the other threads.
            let index = cursor.fetch_add(1, Ordering::SeqCst);
            let Some(input) = inputs.get(index) else {
                break;
            };
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| transform(input)))
                    .unwrap_or_else(|_| Err(panicked()));
            if result.is_err() {
                // Stop handing out work; inputs already in flight still finish and are stored.
                cursor.fetch_max(inputs.len(), Ordering::SeqCst);
            }
            *lock(&slots[index]) = Some(result);
        }
    };
    std::thread::scope(|scope| {
        // The closure only captures shared references, so every worker gets a copy of it.
        let handles: Vec<_> = (0..workers).map(|_| scope.spawn(worker)).collect();
        // Join every worker before propagating a failure so no thread is still running.
        handles
            .into_iter()
            .try_for_each(|handle| handle.join().map_err(|_| panicked()))
    })?;
    // Slots keep input order, so the lowest failing input reports first whichever thread hit it,
    // and every input before it was claimed and finished.
    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(PoisonError::into_inner)
                .unwrap_or_else(|| Err(anyhow!("build worker skipped an input")))
        })
        .collect()
}

fn panicked() -> anyhow::Error {
    anyhow!("build worker panicked")
}

fn lock<T>(slot: &Mutex<T>) -> MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
thread_local! {
    static PINNED_WORKERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn pinned_workers() -> Option<usize> {
    let workers = PINNED_WORKERS.get();
    (workers > 0).then_some(workers)
}

/// Runs `f` with every build on this thread using `workers` threads, whatever the machine or
/// the environment says. Tests compare a serial build against a parallel one this way without
/// mutating process-wide state; nested pins restore the previous value.
#[cfg(test)]
pub(super) fn with_workers<R>(workers: usize, f: impl FnOnce() -> R) -> R {
    struct Restore(usize);
    impl Drop for Restore {
        fn drop(&mut self) {
            PINNED_WORKERS.set(self.0);
        }
    }
    let _restore = Restore(PINNED_WORKERS.replace(workers));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn workers_overlap_and_preserve_order() {
        for requested in [4, 32] {
            let inputs = (0..32).collect::<Vec<_>>();
            let workers = requested.min(8);
            let barrier = Barrier::new(workers);
            let active = AtomicUsize::new(0);
            let peak = AtomicUsize::new(0);
            let output = map_with_workers(
                &inputs,
                |value| {
                    if value % (inputs.len() / workers) == 0 {
                        let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(count, Ordering::SeqCst);
                        barrier.wait();
                        active.fetch_sub(1, Ordering::SeqCst);
                    }
                    Ok(value * 2)
                },
                requested,
            )
            .unwrap();
            assert_eq!(peak.load(Ordering::SeqCst), workers);
            assert_eq!(
                output,
                inputs.iter().map(|value| value * 2).collect::<Vec<_>>()
            );
        }
    }

    /// A contiguous partition would hand inputs 0..4 to one thread, so input 0 waiting for every
    /// other input to finish could never complete. Stealing lets the other workers drain them.
    #[test]
    fn slow_inputs_do_not_idle_other_workers() {
        let completed = AtomicUsize::new(0);
        let started = std::time::Instant::now();
        let output = map_with_workers(
            &(0..16).collect::<Vec<_>>(),
            |value| {
                if *value == 0 {
                    while completed.load(Ordering::SeqCst) < 15 {
                        assert!(
                            started.elapsed() < std::time::Duration::from_secs(20),
                            "other workers never picked up the remaining inputs"
                        );
                        std::thread::yield_now();
                    }
                } else {
                    completed.fetch_add(1, Ordering::SeqCst);
                }
                Ok(*value)
            },
            4,
        )
        .unwrap();
        assert_eq!(output, (0..16).collect::<Vec<_>>());
    }

    /// Four workers hold inputs 0..4 at the barrier before inputs 0 and 2 fail. The two others
    /// still finish before the error returns, no worker is left running, and the lowest failing
    /// input reports whichever thread reached it first. How many later inputs start before the
    /// cursor stops depends on scheduling, so only the guaranteed in-flight work is counted.
    #[test]
    fn workers_finish_before_errors_return() {
        let barrier = Barrier::new(4);
        let completed = AtomicUsize::new(0);
        let running = AtomicUsize::new(0);
        let error = map_with_workers(
            &(0..16).collect::<Vec<_>>(),
            |value| {
                running.fetch_add(1, Ordering::SeqCst);
                if *value < 4 {
                    barrier.wait();
                }
                let result = match *value {
                    0 => Err(anyhow!("first item failed")),
                    2 => Err(anyhow!("third item failed")),
                    value => {
                        completed.fetch_add(1, Ordering::SeqCst);
                        Ok(value)
                    }
                };
                running.fetch_sub(1, Ordering::SeqCst);
                result
            },
            4,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "first item failed");
        assert_eq!(running.load(Ordering::SeqCst), 0);
        assert!(completed.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn panics_become_errors_for_small_and_parallel_builds() {
        for count in [1, 16] {
            let result = map_with_workers(
                &(0..count).collect::<Vec<_>>(),
                |value| -> Result<usize> {
                    if *value == 0 || *value == 8 {
                        panic!("bad render")
                    }
                    Ok(*value)
                },
                4,
            );
            assert!(result.unwrap_err().to_string().contains("panicked"));
        }
    }

    #[test]
    fn worker_override_accepts_only_positive_integers() {
        assert_eq!(parse_workers("4"), Some(4));
        assert_eq!(parse_workers(" 12 \n"), Some(12));
        assert_eq!(parse_workers("0"), None);
        assert_eq!(parse_workers("-1"), None);
        assert_eq!(parse_workers("two"), None);
        assert_eq!(parse_workers(""), None);
    }

    #[test]
    fn pinned_workers_take_precedence_and_stay_bounded() {
        assert_eq!(with_workers(1, workers), 1);
        assert_eq!(with_workers(4, workers), 4);
        assert_eq!(with_workers(99, workers), MAX_WORKERS);
        assert_eq!(
            with_workers(2, || (workers(), with_workers(3, workers), workers())),
            (2, 3, 2)
        );
        assert!(pinned_workers().is_none());
        assert!((1..=MAX_WORKERS).contains(&workers()));
    }
}
