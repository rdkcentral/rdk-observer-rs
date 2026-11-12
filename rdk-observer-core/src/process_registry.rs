//! Registry of process targets and collection state.
//!
//! This module will maintain selected targets using PID and the start time
//! reported in `/proc/<pid>/stat`, following ADR-0002. PID reuse will not attach
//! an old target's collection state to a new process.
//!
//! `process_worker` will use the registry to add, update, and remove targets,
//! deduplicate discovery results and events, and track collection task handles.
//! `collector_worker` will use registered identities to collect one process or
//! sample the selected list. Observation records will belong to `output_*`.

use crate::output_model::{CpuRecord, MemoryRecord, StorageRecord};
use crate::process_worker::{ProcessId, ProcessInfo};
use hashlink::LinkedHashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

////////////////////////////////////////////////////////////////////////////////
/// Number of registry events kept for subscribers before they start lagging
const EVENTS_CAPACITY: usize = 1024;

/// Number of finished processes kept in history before the oldest are dropped
const FINISHED_PROCESSES_CAPACITY: usize = 256;

////////////////////////////////////////////////////////////////////////////////
/// Full process identity, PID alone can be reused by a newer process
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ProcessKey {
    pub pid: ProcessId,
    pub start_time: u64,
}

impl From<&ProcessInfo> for ProcessKey {
    fn from(value: &ProcessInfo) -> Self {
        Self {
            pid: value.pid,
            start_time: value.start_time,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Registered process metadata
#[derive(Debug, Clone)]
pub struct RegisteredProcess {
    key: ProcessKey,
    command: String,
    kernel_thread: bool,
    pub exit_result: Option<Result<u32, u32>>,
    pub last_io_record: Option<(Duration, StorageRecord)>,
    pub last_cpu_record: Option<(Duration, CpuRecord)>,
    pub last_memory_record: Option<(Duration, MemoryRecord)>,
}

impl RegisteredProcess {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn key(&self) -> ProcessKey {
        self.key
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn command(&self) -> &str {
        &self.command
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn kernel_thread(&self) -> bool {
        self.kernel_thread
    }
}

impl From<ProcessInfo> for RegisteredProcess {
    ////////////////////////////////////////////////////////////////////////////////
    fn from(value: ProcessInfo) -> Self {
        Self {
            key: ProcessKey::from(&value),
            command: value.command,
            kernel_thread: value.kernel_thread,
            exit_result: value.exit_result,
            last_io_record: None,
            last_cpu_record: None,
            last_memory_record: None,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Registry change, delivered to every subscriber in the order changes were applied
#[derive(Debug, Clone)]
pub enum ProcessRegistryEvent {
    Started(ProcessKey),
    Updated(ProcessKey),
    Finished(ProcessKey),
}

////////////////////////////////////////////////////////////////////////////////
/// Registry shared across other components.
/// Locks are never held across `.await` or blocking operations (like fs).
#[derive(Debug, Clone)]
pub struct ProcessRegistry {
    state: Arc<RwLock<ProcessRegistryState>>,
}

#[derive(Debug)]
struct ProcessRegistryState {
    /// currently alive processes
    processes: HashMap<ProcessKey, RegisteredProcess>,
    /// history of recently finished processes, oldest first
    finished_processes: LinkedHashMap<ProcessKey, RegisteredProcess>,
    /// broadcast events about process changes
    events_tx: tokio::sync::broadcast::Sender<ProcessRegistryEvent>,
}

impl ProcessRegistry {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn new() -> (Self, ProcessRegistryWriter) {
        let this = Self {
            state: Arc::new(RwLock::new(ProcessRegistryState {
                processes: Default::default(),
                finished_processes: LinkedHashMap::with_capacity(FINISHED_PROCESSES_CAPACITY),
                events_tx: tokio::sync::broadcast::Sender::new(EVENTS_CAPACITY),
            })),
        };
        (this.clone(), ProcessRegistryWriter(this))
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Subscribes to registry changes made after the returned snapshot
    pub fn subscribe_with_snapshot(
        &self,
    ) -> (
        Vec<RegisteredProcess>,
        tokio::sync::broadcast::Receiver<ProcessRegistryEvent>,
    ) {
        let state = self.state.read();
        let snapshot = state.processes.values().cloned().collect();
        (snapshot, state.events_tx.subscribe())
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Gets a clone of process by key from the registry
    pub fn get_process(&self, key: &ProcessKey) -> Option<RegisteredProcess> {
        self.state.read().processes.get(key).cloned()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Update process info in-place without emitting event
    /// It is critical for these updates to be as very fast as possible
    pub fn update_process<F, T>(&self, key: &ProcessKey, f: F) -> Option<T>
    where
        F: FnOnce(&mut RegisteredProcess) -> T,
    {
        self.state.write().processes.get_mut(key).map(f)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Returns a copy of all registered processes
    pub fn get_snapshot(&self) -> Vec<RegisteredProcess> {
        self.state.read().processes.values().cloned().collect()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Returns a copy of recently finished process info
    pub fn get_finished_process(&self, key: &ProcessKey) -> Option<RegisteredProcess> {
        let state = self.state.read();
        state.finished_processes.get(key).cloned()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Returns a copy of finished processes history, oldest first
    pub fn get_finished_snapshot(&self) -> Vec<RegisteredProcess> {
        let state = self.state.read();
        state.finished_processes.values().cloned().collect()
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Writer part of the registry that allows to add/remove processes
#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct ProcessRegistryWriter(ProcessRegistry);

impl ProcessRegistryWriter {
    ////////////////////////////////////////////////////////////////////////////////
    /// Registers a process, returns `false` if this identity is already registered
    pub fn register_process(&self, process: &ProcessInfo) -> bool {
        let key = ProcessKey::from(process);
        let state = &mut *self.state.write();

        let Entry::Vacant(entry) = state.processes.entry(key) else {
            warn!("duplicated pid/start_time detected for {process:?}");
            return false;
        };

        let registered = match state.finished_processes.remove(&key) {
            Some(mut e) => {
                e.command.clone_from(&process.command);
                e.exit_result = process.exit_result;
                e.kernel_thread = process.kernel_thread;
                e
            }
            None => RegisteredProcess::from(process.clone()),
        };

        entry.insert(registered);
        state.notify(ProcessRegistryEvent::Started(key));
        true
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Register an unverified process as a finished process
    pub fn register_unverified_process(&self, process: ProcessInfo) {
        let process = RegisteredProcess::from(process);
        self.state.write().push_finished_process(process);
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Updates metadata of a registered process, unknown identities are ignored
    ///
    /// Returns false if process was not found
    pub fn update_process(&self, process: &ProcessInfo) -> bool {
        let key = ProcessKey::from(process);
        let state = &mut *self.state.write();

        let Some(e) = state.processes.get_mut(&key) else {
            return false;
        };

        let mut changed = false;
        if e.command != process.command {
            changed = true;
            e.command.clone_from(&process.command);
        }
        if e.kernel_thread != process.kernel_thread {
            changed = true;
            e.kernel_thread = process.kernel_thread;
        }

        if changed {
            state.notify(ProcessRegistryEvent::Updated(key));
        }
        true
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Removes a process only if its full identity matches,
    /// finished process is kept in history, oldest entries are dropped when full
    pub fn unregister_process(&self, process: &ProcessInfo) -> Option<RegisteredProcess> {
        let key = ProcessKey::from(process);
        let state = &mut *self.state.write();

        let mut removed = state.processes.remove(&key)?;
        removed.exit_result = process.exit_result;

        state.push_finished_process(removed.clone());
        state.notify(ProcessRegistryEvent::Finished(removed.key));
        Some(removed)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Removes all processes and notifies subscribers
    /// Processes will not be added to finished list because they might still be alive
    pub fn clear_all_processes(&self) {
        let state = &mut *self.state.write();
        for process in std::mem::take(&mut state.processes).into_values() {
            state.notify(ProcessRegistryEvent::Finished(process.key));
        }
    }
}

impl Deref for ProcessRegistryWriter {
    type Target = ProcessRegistry;

    ////////////////////////////////////////////////////////////////////////////////
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ProcessRegistryState {
    ////////////////////////////////////////////////////////////////////////////////
    fn notify(&self, event: ProcessRegistryEvent) {
        // broadcast channel can only fail when there is no active subscribers
        // this is a valid case and doesn't require us to do anything
        let _ = self.events_tx.send(event);
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn push_finished_process(&mut self, finished: RegisteredProcess) {
        if self.finished_processes.len() >= FINISHED_PROCESSES_CAPACITY {
            self.finished_processes.pop_front();
        }
        self.finished_processes.insert(finished.key, finished);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::Receiver;
    use tokio::sync::broadcast::error::TryRecvError;

    fn process(key: ProcessKey, command: &str) -> ProcessInfo {
        ProcessInfo {
            pid: key.pid,
            command: command.to_string(),
            kernel_thread: false,
            start_time: key.start_time,
            exit_result: None,
        }
    }

    fn key(pid: i32, start_time: u64) -> ProcessKey {
        ProcessKey {
            pid: ProcessId(pid),
            start_time,
        }
    }

    fn drain(rx: &mut Receiver<ProcessRegistryEvent>) -> Vec<ProcessRegistryEvent> {
        let mut events = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Empty) => return events,
                Err(e) => panic!("unexpected receive error: {e:?}"),
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn pid_reuse_does_not_inherit_collected_state() {
        let (registry, writer) = ProcessRegistry::new();
        let (_, mut rx) = registry.subscribe_with_snapshot();

        let old_key = key(100, 1);
        let old = process(old_key, "old");

        let new_key = key(100, 2);
        let new = process(new_key, "new");

        // old process collects data and exits, then its PID is reused
        assert!(writer.register_process(&old));
        registry.update_process(&old_key, |e| {
            e.last_memory_record = Some((Duration::default(), MemoryRecord::default()));
        });
        writer.unregister_process(&ProcessInfo {
            exit_result: Some(Ok(0)),
            ..old.clone()
        });
        assert!(writer.register_process(&new));

        // new process starts with clean state, old one is no longer alive
        let registered = registry.get_process(&new_key).unwrap();
        assert_eq!(registered.command(), new.command);
        assert!(registered.last_memory_record.is_none());
        assert!(registered.exit_result.is_none());
        assert!(registry.get_process(&old_key).is_none());

        // old process keeps its collection state and exit result in history
        let finished = registry.get_finished_process(&old_key).unwrap();
        assert_eq!(finished.command(), old.command);
        assert!(finished.last_memory_record.is_some());
        assert_eq!(finished.exit_result, Some(Ok(0)));

        // subscriber sees the old process exit before the new one starts
        let events = drain(&mut rx);
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], ProcessRegistryEvent::Started(e) if e == old_key));
        assert!(matches!(events[1], ProcessRegistryEvent::Finished(e) if e == old_key));
        assert!(matches!(events[2], ProcessRegistryEvent::Started(e) if e == new_key));
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn pid_reuse_with_missed_exit_keeps_identities_separate() {
        let (registry, writer) = ProcessRegistry::new();
        let (_, mut rx) = registry.subscribe_with_snapshot();

        let old_key = key(100, 1);
        let old = process(old_key, "old");

        let new_key = key(100, 2);
        let new = process(new_key, "new");

        // exit of the old process was missed, the new one reuses its PID
        assert!(writer.register_process(&old));
        assert!(writer.register_process(&new));
        assert_eq!(registry.get_snapshot().len(), 2);

        // updates and removals must match the full identity
        writer.update_process(&process(new_key, "renamed"));
        assert_eq!(registry.get_process(&old_key).unwrap().command(), "old");
        assert_eq!(registry.get_process(&new_key).unwrap().command(), "renamed");

        // old identity is removed only once, the new process stays alive
        assert!(writer.unregister_process(&old).is_some());
        assert!(writer.unregister_process(&old).is_none());
        assert!(registry.get_process(&old_key).is_none());
        assert!(registry.get_process(&new_key).is_some());

        // subscriber receives every change in order
        let events = drain(&mut rx);
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], ProcessRegistryEvent::Started(e) if e == old_key));
        assert!(matches!(events[1], ProcessRegistryEvent::Started(e) if e == new_key));
        assert!(matches!(events[2], ProcessRegistryEvent::Updated(e) if e == new_key));
        assert!(matches!(events[3], ProcessRegistryEvent::Finished(e) if e == old_key));
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn finished_history_evicts_oldest_entries() {
        let (registry, writer) = ProcessRegistry::new();
        let total = FINISHED_PROCESSES_CAPACITY + 3;

        // every process starts and finishes, moving into history
        for pid in 0..total as i32 {
            let process = process(key(pid, 1), "p");
            assert!(writer.register_process(&process));
            assert!(writer.unregister_process(&process).is_some());
        }

        // history never grows beyond its capacity
        let history = registry.get_finished_snapshot();
        assert_eq!(history.len(), FINISHED_PROCESSES_CAPACITY);

        // oldest entries were dropped, order is preserved, oldest first
        assert_eq!(history.first().unwrap().key(), key(3, 1));
        assert_eq!(history.last().unwrap().key(), key(total as i32 - 1, 1));
        for pid in 0..3 {
            assert!(registry.get_finished_process(&key(pid, 1)).is_none());
        }
        assert!(registry.get_finished_process(&key(3, 1)).is_some());
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn finished_history_eviction_includes_unverified_processes() {
        let (registry, writer) = ProcessRegistry::new();

        // one more unverified process than history can hold
        for pid in 0..(FINISHED_PROCESSES_CAPACITY + 1) as i32 {
            writer.register_unverified_process(process(key(pid, 1), "unverified"));
        }

        // history never grows beyond its capacity
        assert_eq!(
            registry.get_finished_snapshot().len(),
            FINISHED_PROCESSES_CAPACITY,
        );

        // oldest unverified entry was dropped to make room for the newest
        assert!(registry.get_finished_process(&key(0, 1)).is_none());
        assert!(registry.get_finished_process(&key(1, 1)).is_some());
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn lagged_subscriber_recovers_with_new_snapshot() {
        let (registry, writer) = ProcessRegistry::new();

        // subscriber starts with an empty registry
        let (snapshot, mut rx) = registry.subscribe_with_snapshot();
        assert!(snapshot.is_empty());

        // overflow the events buffer while the subscriber is not reading
        let missed = 10;
        let total = EVENTS_CAPACITY + missed;
        for pid in 0..total as i32 {
            assert!(writer.register_process(&process(key(pid, 1), "p")));
        }

        // subscriber is notified that the oldest events were lost
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Lagged(n)) if n == missed as u64));

        // subscriber resynchronizes by taking a new snapshot with a new receiver
        let (snapshot, mut rx) = registry.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), total);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        // registry changes after the snapshot was taken
        writer.unregister_process(&process(key(0, 1), "p"));
        assert!(writer.register_process(&process(key(total as i32, 1), "p")));

        // both changes are received in order
        let events = drain(&mut rx);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ProcessRegistryEvent::Finished(k) if k == key(0, 1)));
        assert!(matches!(events[1], ProcessRegistryEvent::Started(k) if k == key(total as i32, 1)));
    }
}
