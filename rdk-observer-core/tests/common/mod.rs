//! Helpers shared by integration tests.
//!
//! Each test binary compiles this module separately and uses only a part of it.
#![allow(dead_code)]

use futures::future::BoxFuture;
use parking_lot::Mutex;
use rdk_observer_core::output_model::{
    BlockDeviceIdentity, CpuCounters, CpuRecord, MemoryRecord, StorageCounters, StorageRecord,
    SystemCpuCounters, SystemCpuRecord, SystemMemoryRecord, SystemStorageCounters,
    SystemStorageRecord,
};
use rdk_observer_core::process_registry::{ProcessKey, ProcessRegistry, ProcessRegistryWriter};
use rdk_observer_core::process_worker::{ProcessId, ProcessInfo};
use rdk_observer_core::runtime::{Runtime, RuntimeConfig, RuntimeContext};
use rdk_observer_core::runtime_components::RuntimeComponents;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Collection interval used by test runtimes
pub const INTERVAL: Duration = Duration::from_millis(10);

/// Maximum time to wait for a condition in [wait_until]
pub const TIMEOUT: Duration = Duration::from_secs(5);

////////////////////////////////////////////////////////////////////////////////
// TestRuntimeComponents
////////////////////////////////////////////////////////////////////////////////
/// Runtime components that return synthetic records and track every call.
///
/// Counters in returned records grow with each read of the same source,
/// so the runtime can derive utilization and rates from consecutive samples.
/// Behavior can be changed while the runtime is running.
/// Clones share the same state.
#[derive(Debug, Clone)]
pub struct TestRuntimeComponents {
    state: Arc<Mutex<TestRuntimeComponentsShared>>,
    /// latest process worker created by the runtime
    process_worker_tx: Arc<tokio::sync::watch::Sender<Option<TestProcessWorker>>>,
}

#[derive(Debug, Default)]
struct TestRuntimeComponentsShared {
    process_worker_runs: usize,
    system_io_reads: usize,
    system_cpu_reads: usize,
    system_memory_reads: usize,
    process_io_reads: Vec<ProcessId>,
    process_cpu_reads: Vec<ProcessKey>,
    process_memory_reads: Vec<ProcessId>,
    processes_with_failing_cpu_read: HashSet<ProcessKey>,
}

impl TestRuntimeComponents {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn new() -> Self {
        Self {
            state: Default::default(),
            process_worker_tx: Arc::new(tokio::sync::watch::Sender::new(None)),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn build_runtime(&self, mut config: RuntimeConfig) -> Runtime {
        config.system_collection_interval = INTERVAL;
        config.process_collection_interval_initial = INTERVAL;
        config.process_collection_interval_ongoing = INTERVAL;
        Runtime::new_with_components(config, TestRuntimeComponentsImpl(self.clone()))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn start_runtime(
        &self,
        config: RuntimeConfig,
    ) -> (ProcessRegistry, JoinHandle<anyhow::Result<()>>) {
        let runtime = self.build_runtime(config);
        let registry = runtime.registry();
        (registry, tokio::spawn(runtime.run()))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Returns the alive process worker or waits until the runtime creates one,
    /// panics on timeout
    pub async fn wait_for_process_worker(&self) -> TestProcessWorker {
        let mut rx = self.process_worker_tx.subscribe();
        let future = tokio::time::timeout(
            TIMEOUT,
            rx.wait_for(|e| e.as_ref().is_some_and(|e| e.is_alive())),
        );

        let worker = future
            .await
            .expect("timed out waiting for process worker")
            .expect("process worker channel is closed");

        worker.clone().unwrap()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn set_process_with_failing_cpu_read(&self, process: ProcessKey, failing: bool) -> &Self {
        let pids = &mut self.state.lock().processes_with_failing_cpu_read;
        if failing {
            pids.insert(process);
        } else {
            pids.remove(&process);
        }
        self
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn process_worker_runs(&self) -> usize {
        self.state.lock().process_worker_runs
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn system_io_reads(&self) -> usize {
        self.state.lock().system_io_reads
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn system_cpu_reads(&self) -> usize {
        self.state.lock().system_cpu_reads
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn system_memory_reads(&self) -> usize {
        self.state.lock().system_memory_reads
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn process_cpu_reads(&self) -> Vec<ProcessKey> {
        self.state.lock().process_cpu_reads.clone()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn process_io_reads_of(&self, process: ProcessId) -> usize {
        let state = self.state.lock();
        let iter = &mut state.process_io_reads.iter();
        iter.filter(|e| **e == process).count()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn process_cpu_reads_of(&self, process: ProcessKey) -> usize {
        let state = self.state.lock();
        let iter = &mut state.process_cpu_reads.iter();
        iter.filter(|e| **e == process).count()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn process_memory_reads_of(&self, process: ProcessId) -> usize {
        let state = self.state.lock();
        let iter = &mut state.process_memory_reads.iter();
        iter.filter(|e| **e == process).count()
    }
}

////////////////////////////////////////////////////////////////////////////////
// TestRuntimeComponentsImpl
////////////////////////////////////////////////////////////////////////////////
#[derive(Debug)]
struct TestRuntimeComponentsImpl(TestRuntimeComponents);

impl RuntimeComponents for TestRuntimeComponentsImpl {
    ////////////////////////////////////////////////////////////////////////////////
    fn process_worker(
        &self,
        context: RuntimeContext,
        registry: ProcessRegistryWriter,
    ) -> BoxFuture<'static, ()> {
        let _ = context;
        let worker = TestProcessWorker::new(self.0.clone(), registry);

        self.0.state.lock().process_worker_runs += 1;
        self.0.process_worker_tx.send_replace(Some(worker.clone()));

        Box::pin(async move {
            let worker = scopeguard::guard(worker, |worker| {
                // remove this worker from active workers
                let channel = &worker.0.components.process_worker_tx;
                channel.send_if_modified(|p| p.take_if(|e| worker.same_as(e)).is_some());

                // process_worker_tx must be reset before marking us as dead
                // this is required for wait_until_dead to function correctly
                worker.0.alive.send_replace(false);
            });

            // wait for kill signal
            worker.0.kill.notified().await;
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn read_system_io(&self) -> anyhow::Result<Vec<SystemStorageRecord>> {
        let n = {
            let mut state = self.0.state.lock();
            state.system_io_reads += 1;
            state.system_io_reads as u64
        };
        Ok(vec![SystemStorageRecord {
            device_identity: BlockDeviceIdentity {
                major: 8,
                minor: 0,
                name: "sda".into(),
            },
            counters: SystemStorageCounters {
                read_operations: Some(n),
                write_operations: Some(n),
                read_sectors: Some(n * 8),
                write_sectors: Some(n * 16),
            },
            rates: None,
        }])
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn read_system_cpu(&self) -> anyhow::Result<SystemCpuRecord> {
        let n = {
            let mut state = self.0.state.lock();
            state.system_cpu_reads += 1;
            state.system_cpu_reads as u64
        };
        Ok(SystemCpuRecord {
            clock_ticks_per_second: 100,
            counters: SystemCpuCounters {
                user_ticks: Some(n),
                idle_ticks: Some(n),
                ..Default::default()
            },
            utilization: None,
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn read_system_memory(&self) -> anyhow::Result<SystemMemoryRecord> {
        self.0.state.lock().system_memory_reads += 1;
        Ok(SystemMemoryRecord {
            total_bytes: Some(1 << 30),
            available_bytes: Some(1 << 29),
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn read_process_io(&self, process: ProcessId) -> anyhow::Result<StorageRecord> {
        let value = {
            let mut state = self.0.state.lock();
            state.process_io_reads.push(process);

            let iter = state.process_io_reads.iter();
            iter.filter(|e| **e == process).count() as u64
        };
        Ok(StorageRecord {
            counters: StorageCounters {
                read_bytes: Some(value * 4096),
                write_bytes: Some(value * 512),
                read_syscalls: Some(value),
                write_syscalls: Some(value),
            },
            rates: None,
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn read_process_cpu(&self, process: ProcessKey) -> anyhow::Result<CpuRecord> {
        let value = {
            let mut state = self.0.state.lock();
            state.process_cpu_reads.push(process);
            if state.processes_with_failing_cpu_read.contains(&process) {
                anyhow::bail!("process {process:?} is gone");
            }
            let iter = state.process_cpu_reads.iter();
            iter.filter(|e| e.pid == process.pid).count() as u64
        };
        Ok(CpuRecord {
            clock_ticks_per_second: 100,
            counters: CpuCounters {
                user_ticks: Some(value + 1),
                system_ticks: Some(0),
            },
            utilization: None,
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn read_process_memory(&self, process: ProcessId) -> anyhow::Result<MemoryRecord> {
        self.0.state.lock().process_memory_reads.push(process);
        Ok(MemoryRecord {
            virtual_bytes: Some(default_process_virtual_bytes(process)),
            resident_bytes: Some(default_process_resident_bytes(process)),
            proportional_bytes: Some(default_process_proportional_bytes(process)),
        })
    }
}

////////////////////////////////////////////////////////////////////////////////
// TestProcessWorker
////////////////////////////////////////////////////////////////////////////////
/// Handle to a process worker created by the runtime.
///
/// Worker does nothing on its own, the test registers processes through
/// [TestProcessWorker::writer] and stops it with [TestProcessWorker::kill].
/// Clones control the same worker.
#[derive(Debug, Clone)]
pub struct TestProcessWorker(Arc<TestProcessWorkerShared>);

#[derive(Debug)]
struct TestProcessWorkerShared {
    components: TestRuntimeComponents,
    writer: ProcessRegistryWriter,
    alive: tokio::sync::watch::Sender<bool>,
    kill: Notify,
}

impl TestProcessWorker {
    ////////////////////////////////////////////////////////////////////////////////
    fn new(components: TestRuntimeComponents, writer: ProcessRegistryWriter) -> Self {
        Self(Arc::new(TestProcessWorkerShared {
            components,
            writer,
            alive: tokio::sync::watch::Sender::new(true),
            kill: Notify::new(),
        }))
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn writer(&self) -> &ProcessRegistryWriter {
        &self.0.writer
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn kill(&self) {
        self.0.kill.notify_one();
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn is_alive(&self) -> bool {
        *self.0.alive.borrow()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn wait_until_dead(&self) {
        let subscriber = &mut self.0.alive.subscribe();
        let future = tokio::time::timeout(TIMEOUT, subscriber.wait_for(|e| !*e));

        if future.await.is_err() {
            panic!("process worker didn't die in {TIMEOUT:?}");
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn same_as(&self, other: &TestProcessWorker) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

////////////////////////////////////////////////////////////////////////////////
// Helpers
////////////////////////////////////////////////////////////////////////////////
pub fn process_key(pid: i32, start_time: u64) -> ProcessKey {
    ProcessKey {
        pid: ProcessId(pid),
        start_time,
    }
}

////////////////////////////////////////////////////////////////////////////////
pub fn process(key: ProcessKey, command: &str) -> ProcessInfo {
    ProcessInfo {
        pid: key.pid,
        command: command.to_string(),
        kernel_thread: false,
        start_time: key.start_time,
        exit_result: None,
    }
}

////////////////////////////////////////////////////////////////////////////////
pub async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let future = tokio::time::timeout(TIMEOUT, async {
        while !condition() {
            tokio::time::sleep(INTERVAL / 2).await;
        }
    });
    assert!(future.await.is_ok(), "timed out waiting until {what}");
}

////////////////////////////////////////////////////////////////////////////////
pub fn default_process_virtual_bytes(process: ProcessId) -> u64 {
    process.0 as u64 * 1024 * 100
}

////////////////////////////////////////////////////////////////////////////////
pub fn default_process_resident_bytes(process: ProcessId) -> u64 {
    process.0 as u64 * 1024 * 10
}

////////////////////////////////////////////////////////////////////////////////
pub fn default_process_proportional_bytes(process: ProcessId) -> u64 {
    process.0 as u64 * 1024
}
