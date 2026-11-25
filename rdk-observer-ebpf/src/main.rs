#![cfg_attr(target_arch = "bpf", no_std)]
#![cfg_attr(target_arch = "bpf", no_main)]

use aya_ebpf::bindings::BPF_NOEXIST;
use aya_ebpf::btf_maps::{HashMap, HashOfMaps, PerCpuArray};
use aya_ebpf::helpers::{bpf_get_current_comm, bpf_get_current_pid_tgid, bpf_ktime_get_ns};
use aya_ebpf::macros::{btf_map, tracepoint};
use aya_ebpf::programs::TracePointContext;
use rdk_observer_ebpf_common::{EbpfThreadStats, MAX_PROCESSES, MAX_THREADS_PER_PROCESS};

////////////////////////////////////////////////////////////////////////////////
///
/// Per-thread collected data of a single process: tid -> stats
/// (ebpf -> userspace)
///
/// This doesn't require per-cpu map because each thread
/// can be active only on a single cpu core at each moment of time
///
type ThreadStatsMap = HashMap<u32, EbpfThreadStats, MAX_THREADS_PER_PROCESS>;

///
/// Processes that should be monitored: pid -> per-thread stats map
/// (userspace -> ebpf)
///
#[btf_map]
static PROCESS_THREADS: HashOfMaps<u32, ThreadStatsMap, MAX_PROCESSES> = HashOfMaps::new();

///
/// Per-cpu timestamp (ns) of the last context switch on this cpu (0 means "unknown")
/// (ebpf internal)
///
#[btf_map]
static SWITCH_IN_NS: PerCpuArray<u64, 1> = PerCpuArray::new();

////////////////////////////////////////////////////////////////////////////////
///
/// sched/sched_switch fires on every context switch, in the context of the
/// outgoing (prev) task, so `current` is the task being switched out.
///
/// The previous sched_switch on the same cpu is exactly the moment prev was
/// switched in, so its on-cpu time is `now - switched_in_ns`.
///
#[tracepoint]
pub fn on_sched_switch(_ctx: TracePointContext) -> u32 {
    on_sched_switch_inner();
    0
}

////////////////////////////////////////////////////////////////////////////////
fn on_sched_switch_inner() {
    let Some(switch_in) = SWITCH_IN_NS.get_ptr_mut(0) else {
        return;
    };

    // SAFETY: per-cpu slot, the program can't be preempted by itself on the same cpu
    let now = unsafe { bpf_ktime_get_ns() };
    let started = unsafe { core::ptr::replace(switch_in, now) };

    let combined_id = bpf_get_current_pid_tgid();
    let thread_id = combined_id as u32;
    let thread_group_id = (combined_id >> 32) as u32;

    // idle (swapper) task, never tracked
    if thread_group_id == 0 {
        return;
    }

    // SAFETY: inner maps are only added/removed by userspace, lookups are RCU protected
    let Some(threads) = (unsafe { PROCESS_THREADS.get(&thread_group_id) }) else {
        return; // missing inner map means process should not be tracked (expected)
    };

    // first switch on this cpu since attach: on-cpu time is unknown
    let run_ns = if started == 0 {
        0
    } else {
        now.saturating_sub(started)
    };

    // switched off thread command
    let comm = bpf_get_current_comm().unwrap_or_default();

    match threads.get_ptr_mut(thread_id) {
        Some(stats) => {
            // SAFETY: a thread is switched out on a single cpu at a time,
            // so there are no concurrent writers for this entry
            let stats = unsafe { &mut *stats };
            stats.run_ns = stats.run_ns.wrapping_add(run_ns);
            stats.switches_out = stats.switches_out.wrapping_add(1);
            stats.comm = comm; // threads may be renamed at any time, keep it fresh
        }
        None => {
            let stats = EbpfThreadStats {
                run_ns,
                switches_out: 1,
                comm,
            };
            // fails only when the inner map is full; nothing to do about it here
            let _ = threads.insert(thread_id, stats, BPF_NOEXIST as u64);
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(target_arch = "bpf")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(not(target_arch = "bpf"))]
fn main() {
    // this is a fix for rust-analyzer / RustRover highlighting
    // without it IDE will show errors while the code successfully builds
}
