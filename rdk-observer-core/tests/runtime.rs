//! Integration tests for [Runtime] with mocked [RuntimeComponents].
//!
//! Components replace procfs readers and process discovery, so the tests drive
//! the real runtime, registry and collector workers without touching the host.

mod common;

use crate::common::default_process_resident_bytes;
use common::{INTERVAL, TestRuntimeComponents, process, process_key, wait_until};

////////////////////////////////////////////////////////////////////////////////
#[tokio::test]
async fn runtime_collects_registered_process() {
    let process_key = process_key(4242, 7);
    let process = process(process_key, "target");

    let components = TestRuntimeComponents::new();
    let (registry, runtime) = components.start_runtime(Default::default());

    let worker = components.wait_for_process_worker().await;
    assert!(worker.writer().register_process(&process));

    // system collection runs independently of process collection
    wait_until("system records are collected", || {
        components.system_io_reads() >= 2
            && components.system_cpu_reads() >= 2
            && components.system_memory_reads() >= 2
    })
    .await;

    // collected records are stored in the registry, utilization needs two samples
    wait_until("process records are collected", || {
        components.process_io_reads_of(process_key.pid) >= 2
            && components.process_cpu_reads_of(process_key) >= 2
            && components.process_memory_reads_of(process_key.pid) >= 2
    })
    .await;

    runtime.abort();

    let registered = registry.get_process(&process_key).unwrap();
    assert_eq!(registered.command(), process.command);
    assert_eq!(
        registered.last_memory_record.unwrap().1.resident_bytes,
        Some(default_process_resident_bytes(process_key.pid)),
    );

    // cpu reads validate the registered identity
    let cpu_reads = components.process_cpu_reads();
    assert!(cpu_reads.iter().all(|e| *e == process_key), "{cpu_reads:?}");

    // healthy process worker is never restarted
    assert_eq!(components.process_worker_runs(), 1);
}

////////////////////////////////////////////////////////////////////////////////
#[tokio::test]
async fn runtime_restarts_process_worker_with_clean_registry() {
    let stale = process_key(100, 1);
    let fresh = process_key(200, 2);

    let components = TestRuntimeComponents::new();
    let (registry, runtime) = components.start_runtime(Default::default());

    let first = components.wait_for_process_worker().await;
    assert!(first.writer().register_process(&process(stale, "stale")));

    // wait for runtime to collect some information
    wait_until("stale process is collected", || {
        components.process_cpu_reads_of(stale) > 0
    })
    .await;

    // killed current process worker
    first.kill();
    first.wait_until_dead().await;

    // wait for second process worker
    let second = components.wait_for_process_worker().await;
    assert!(!first.is_alive());
    assert!(!second.same_as(&first));
    assert_eq!(components.process_worker_runs(), 2);

    // state of the killed worker is dropped, its processes might still be alive
    assert!(registry.get_process(&stale).is_none());
    assert!(registry.get_finished_process(&stale).is_none());

    // restarted worker registers its processes and collection picks them up
    assert!(second.writer().register_process(&process(fresh, "fresh")));
    wait_until("fresh process is collected", || {
        registry
            .get_process(&fresh)
            .is_some_and(|e| e.last_memory_record.is_some())
    })
    .await;

    // collection of the dropped process is stopped
    let stale_reads = components.process_cpu_reads_of(stale);
    tokio::time::sleep(INTERVAL * 10).await;
    assert_eq!(components.process_cpu_reads_of(stale), stale_reads);

    runtime.abort();
}

////////////////////////////////////////////////////////////////////////////////
#[tokio::test]
async fn runtime_skips_process_collectors_after_failed_cpu_read() {
    let gone = process_key(1, 1);
    let alive = process_key(2, 2);

    let components = TestRuntimeComponents::new();
    components.set_process_with_failing_cpu_read(gone, true);
    let (registry, runtime) = components.start_runtime(Default::default());

    // register processes
    let worker = components.wait_for_process_worker().await;
    assert!(worker.writer().register_process(&process(gone, "gone")));
    assert!(worker.writer().register_process(&process(alive, "alive")));

    // failing process keeps being retried while others are collected normally
    wait_until("both processes are sampled several times", || {
        components.process_cpu_reads_of(gone) >= 3
            && components.process_memory_reads_of(alive.pid) >= 3
    })
    .await;

    runtime.abort();

    // cpu read validates identity, other collectors are skipped when it fails
    assert_eq!(components.process_io_reads_of(gone.pid), 0);
    assert_eq!(components.process_memory_reads_of(gone.pid), 0);

    let gone = registry.get_process(&gone).unwrap();
    assert!(gone.last_cpu_record.is_none());
    assert!(gone.last_io_record.is_none());
    assert!(gone.last_memory_record.is_none());

    let alive = registry.get_process(&alive).unwrap();
    assert!(alive.last_cpu_record.is_some());
    assert!(alive.last_io_record.is_some());
    assert!(alive.last_memory_record.is_some());
}
