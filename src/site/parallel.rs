//! Ordered CPU work with bounded scoped workers and a cheap path for small builds.

use anyhow::Result;

pub(super) fn map<T, U, F>(inputs: &[T], transform: F) -> Result<Vec<U>>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> Result<U> + Sync,
{
    let workers = std::thread::available_parallelism().map_or(1, usize::from);
    map_with_workers(inputs, transform, workers)
}

fn map_with_workers<T, U, F>(inputs: &[T], transform: F, workers: usize) -> Result<Vec<U>>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> Result<U> + Sync,
{
    let workers = workers.clamp(1, 8).min(inputs.len().max(1));
    if workers == 1 || inputs.len() < 8 {
        return std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            inputs.iter().map(transform).collect()
        }))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("build worker panicked")));
    }
    let transform = &transform;
    let chunks = std::thread::scope(|scope| {
        let workers = inputs
            .chunks(inputs.len().div_ceil(workers))
            .map(|chunk| {
                scope.spawn(move || chunk.iter().map(transform).collect::<Result<Vec<_>>>())
            })
            .collect::<Vec<_>>();
        // Join every worker before propagating a failure, including panics in later chunks.
        workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("build worker panicked")))
            })
            .collect::<Vec<_>>()
    });
    let mut output = Vec::with_capacity(inputs.len());
    for chunk in chunks {
        output.extend(chunk?);
    }
    Ok(output)
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

    #[test]
    fn workers_finish_before_errors_return() {
        let completed = AtomicUsize::new(0);
        let error = map_with_workers(
            &(0..16).collect::<Vec<_>>(),
            |value| {
                if *value == 0 {
                    anyhow::bail!("first item failed");
                }
                completed.fetch_add(1, Ordering::SeqCst);
                Ok(*value)
            },
            4,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "first item failed");
        assert_eq!(completed.load(Ordering::SeqCst), 12);
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
}
