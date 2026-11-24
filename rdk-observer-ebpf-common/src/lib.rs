#![no_std]

////////////////////////////////////////////////////////////////////////////////
/// Max number of processes tracked at the same time
pub const MAX_PROCESSES: usize = 16;

/// Max number of threads tracked per process
pub const MAX_THREADS_PER_PROCESS: usize = 1024;

////////////////////////////////////////////////////////////////////////////////
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct EbpfThreadStats {
    pub run_ns: u64,
    pub switches_out: u64,
    pub comm: [u8; 16],
}

#[cfg(feature = "userspace")]
unsafe impl aya::Pod for EbpfThreadStats {}
