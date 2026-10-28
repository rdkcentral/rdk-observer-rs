use crate::output_model::{MemoryRecord, SystemMemoryRecord};
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
    fn read_system_memory(&self) -> anyhow::Result<SystemMemoryRecord> {
        crate::collector_memory::read_system_memory()
    }

    fn read_process_memory(&self, pid: i32) -> anyhow::Result<MemoryRecord> {
        crate::collector_memory::read_process_memory(pid)
    }
}
