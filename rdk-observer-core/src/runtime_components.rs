use crate::output_model::{
    CpuRecord, MemoryRecord, StorageRecord, SystemCpuRecord, SystemMemoryRecord,
    SystemStorageRecord,
};
use crate::process_registry::{ProcessKey, ProcessRegistryWriter};
use crate::process_worker::ProcessId;
use crate::runtime::RuntimeContext;
use futures::future::BoxFuture;
use std::any::Any;
use std::fmt::Debug;

/// Default runtime components provider
#[derive(Debug)]
pub struct DefaultRuntimeComponents;

impl RuntimeComponents for DefaultRuntimeComponents {}

/// Runtime components provider used for DI
///
/// This trait is used to override runtime components for different use-cases, like testing
pub trait RuntimeComponents: Any + Send + Sync + Debug {
    fn process_worker(
        &self,
        context: RuntimeContext,
        registry: ProcessRegistryWriter,
    ) -> BoxFuture<'static, ()> {
        Box::pin(crate::process_worker::ProcessWorker::run(context, registry))
    }

    fn read_system_io(&self) -> anyhow::Result<Vec<SystemStorageRecord>> {
        crate::collector_io::read_system_io()
    }

    fn read_system_cpu(&self) -> anyhow::Result<SystemCpuRecord> {
        crate::collector_cpu::read_system_cpu()
    }

    fn read_system_memory(&self) -> anyhow::Result<SystemMemoryRecord> {
        crate::collector_memory::read_system_memory()
    }

    fn read_process_io(&self, process: ProcessId) -> anyhow::Result<StorageRecord> {
        crate::collector_io::read_process_io(process.0)
    }

    fn read_process_cpu(&self, process: ProcessKey) -> anyhow::Result<CpuRecord> {
        crate::collector_cpu::read_process_cpu(process.pid.0, process.start_time)
    }

    fn read_process_memory(&self, process: ProcessId) -> anyhow::Result<MemoryRecord> {
        crate::collector_memory::read_process_memory(process.0)
    }
}
