//! Process target discovery and reconciliation worker.
//!
//! This module will run a worker started by `runtime` to discover candidate
//! targets in `normal-observer-mode` through an initial procfs inventory snapshot,
//! full reconciliation scans every 30 seconds, and netlink process connector
//! fork, exec, and exit events between scans. Subscribe to available connector
//! events before the startup scan and consume them while scanning.
//!
//! Discovery results and events will be forwarded to `process_worker`, which
//! will apply selection rules, validate identities, update `process_registry`,
//! and coordinate collection. Discovery scans will identify process targets;
//! their resource measurements will be collected by `collector_*`.
//!
//! ADR-0003 defines `NETLINK_CONNECTOR` / `CN_PROC` as the normal lifecycle
//! source. Reconcile scans with concurrent events and distinguish task exits
//! from whole-process completion. Partial scans must not imply target exits.
//! Detected event loss will request an early, coalesced reconciliation. If the
//! connector is unavailable, retain 30-second scans, report reduced coverage,
//! and retry subscription with bounded backoff. Short-lived processes may be
//! missed when events are unavailable or lost.
//!
//! Discovery and reconciliation will continue during `forensic-observer-mode`.
//! eBPF will be reserved for anomaly-triggered process or thread analysis, with
//! no eBPF programs loaded or attached for normal discovery. The reconciliation
//! cadence is independent of resource sampling intervals.

use libc::ENOBUFS;
use neli::connector::{CnMsg, CnMsgBuilder, ProcEvent, ProcEventHeader};
use neli::consts::connector::{CnMsgIdx, CnMsgVal, ProcCnMcastOp};
use neli::consts::nl::{NlmF, Nlmsg};
use neli::consts::socket::NlFamily;
use neli::err::SocketError;
use neli::nl::{NlPayload, NlmsghdrBuilder};
use neli::socket::asynchronous::NlSocketHandle;
use neli::utils::Groups;
use rustix::time::{ClockId, Timespec, clock_gettime};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task::JoinSet;
use tracing::{debug, error, instrument, trace, warn};

////////////////////////////////////////////////////////////////////////////////
const RECONNECT_TIMEOUT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_TIMEOUT_MAX: Duration = Duration::from_mins(1);
const LISTEN_ACK_TIMEOUT: Duration = Duration::from_secs(5);

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug)]
pub enum NetlinkProcessEvent {
    Fork(NetlinkProcessForkEvent),
    Exec(NetlinkProcessExecEvent),
    Comm(NetlinkProcessCommEvent),
    Exit(NetlinkProcessExitEvent),
}

#[derive(Debug)]
pub struct NetlinkProcessForkEvent {
    pub child_pid: i32,
    pub child_tgid: i32,
    pub ts_boot_ticks: u64,
}

#[derive(Debug)]
pub struct NetlinkProcessExecEvent {
    pub process_pid: i32,
    pub process_tgid: i32,
    pub ts_boot_ticks: u64,
}

#[derive(Debug)]
pub struct NetlinkProcessCommEvent {
    pub process_pid: i32,
    pub process_tgid: i32,
    pub command: String,
    pub ts_boot_ticks: u64,
}

#[derive(Debug)]
pub struct NetlinkProcessExitEvent {
    pub process_pid: i32,
    pub process_tgid: i32,
    pub ts_boot_ticks: u64,
    pub exit_result: Result<u32, u32>,
}

////////////////////////////////////////////////////////////////////////////////
pub struct NetlinkProcessDiscovery {
    pub connected: tokio::sync::watch::Receiver<bool>,
    pub refresh: Arc<Notify>,
    pub events: tokio::sync::mpsc::Receiver<NetlinkProcessEvent>,
    join_set: JoinSet<()>,
}

impl Default for NetlinkProcessDiscovery {
    ////////////////////////////////////////////////////////////////////////////////
    fn default() -> Self {
        let (connected_tx, connected_rx) = tokio::sync::watch::channel(false);
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(1024);

        let worker = NetlinkProcessDiscoveryWorker {
            connected: connected_tx,
            refresh: Default::default(),
            events: events_tx,
            timeout: RECONNECT_TIMEOUT_MIN,
        };

        let mut this = Self {
            connected: connected_rx,
            refresh: worker.refresh.clone(),
            events: events_rx,
            join_set: Default::default(),
        };
        this.join_set.spawn(worker.run());
        this
    }
}

////////////////////////////////////////////////////////////////////////////////
struct NetlinkProcessDiscoveryWorker {
    connected: tokio::sync::watch::Sender<bool>,
    refresh: Arc<Notify>,
    events: tokio::sync::mpsc::Sender<NetlinkProcessEvent>,
    timeout: Duration,
}

impl NetlinkProcessDiscoveryWorker {
    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(skip_all, "proc_discovery")]
    async fn run(mut self) {
        loop {
            debug!("opening connector socket");
            if let Some(socket) = Self::open_and_register_socket().await {
                debug!("socket opened");
                self.connected.send_replace(true);

                loop {
                    if let Err(e) = self.process_events(&socket).await {
                        break error!("failed to read from socket: {e}");
                    }
                }

                warn!("socket closed, events no longer processing");
                self.connected.send_replace(false);
            }

            debug!("reconnecting in {:?}", self.timeout);
            tokio::time::sleep(self.timeout).await;
            self.timeout = (self.timeout * 2).min(RECONNECT_TIMEOUT_MAX);
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn process_events(&mut self, socket: &NlSocketHandle) -> anyhow::Result<()> {
        let messages = match socket.recv::<Nlmsg, CnMsg<ProcEventHeader>>().await {
            Ok((e, _)) => {
                self.timeout = RECONNECT_TIMEOUT_MIN;
                e
            }
            Err(SocketError::Io(e)) if e.raw_os_error() == Some(ENOBUFS) => {
                warn!("events lost, requesting full refresh");
                self.timeout = RECONNECT_TIMEOUT_MIN;
                self.refresh.notify_one();
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };

        for message in messages {
            let Some(message) = message.ok() else {
                continue;
            };
            let Some(payload) = message.get_payload() else {
                continue;
            };
            self.process_message(payload.payload()).await;
        }
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn process_message(&mut self, payload: &ProcEventHeader) {
        let event = match payload.event {
            ProcEvent::Fork {
                child_pid: pid,
                child_tgid: tgid,
                ..
            } => {
                let ts_boot_ticks = monotonic_ns_to_boot_ticks(payload.timestamp_ns);

                trace!(pid, tgid, ts_boot_ticks, "fork");

                NetlinkProcessEvent::Fork(NetlinkProcessForkEvent {
                    child_pid: pid,
                    child_tgid: tgid,
                    ts_boot_ticks,
                })
            }

            ProcEvent::Exec {
                process_pid: pid,
                process_tgid: tgid,
            } => {
                let ts_boot_ticks = monotonic_ns_to_boot_ticks(payload.timestamp_ns);

                trace!(pid, tgid, ts_boot_ticks, "exec");

                NetlinkProcessEvent::Exec(NetlinkProcessExecEvent {
                    process_pid: pid,
                    process_tgid: tgid,
                    ts_boot_ticks,
                })
            }

            ProcEvent::Comm {
                process_pid: pid,
                process_tgid: tgid,
                comm,
            } => {
                let ts_boot_ticks = monotonic_ns_to_boot_ticks(payload.timestamp_ns);
                let command = comm.split(|e| *e == b'\0').next().unwrap_or(&comm);
                let command = String::from_utf8_lossy(command).into_owned();

                trace!(pid, tgid, ts_boot_ticks, command, "comm");

                NetlinkProcessEvent::Comm(NetlinkProcessCommEvent {
                    process_pid: pid,
                    process_tgid: tgid,
                    command,
                    ts_boot_ticks,
                })
            }

            ProcEvent::Exit {
                process_pid: pid,
                process_tgid: tgid,
                exit_code,
                ..
            } => {
                let ts_boot_ticks = monotonic_ns_to_boot_ticks(payload.timestamp_ns);
                let exit_result = match exit_code & 0x7f {
                    0 => Ok((exit_code >> 8) & 0xff),
                    signal => Err(signal),
                };

                trace!(pid, tgid, ts_boot_ticks, ?exit_result, "exit");

                NetlinkProcessEvent::Exit(NetlinkProcessExitEvent {
                    process_pid: pid,
                    process_tgid: tgid,
                    exit_result,
                    ts_boot_ticks,
                })
            }

            _ => return,
        };

        if let Err(TrySendError::Full(_)) = self.events.try_send(event) {
            warn!("failed to send event, channel is full");
            self.refresh.notify_one();
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn open_and_register_socket() -> Option<NlSocketHandle> {
        let groups = Groups::new_bitmask(CnMsgIdx::Proc.into());
        let socket = NlSocketHandle::connect(NlFamily::Connector, None, groups)
            .inspect_err(|e| error!("failed to open socket: {e}"))
            .ok()?;

        // kernel overwrites `seq` of the ack with its own event counter,
        // so only `ack` and `ack + 1` back can identify our request.
        let request_listen_ack = socket.pid();
        let response_listen_ack = request_listen_ack.wrapping_add(1);

        let listen_payload = CnMsgBuilder::default()
            .idx(CnMsgIdx::Proc)
            .val(CnMsgVal::Proc)
            .ack(request_listen_ack)
            .payload(ProcCnMcastOp::Listen)
            .build()
            .inspect_err(|e| error!("failed to build listen payload: {e}"))
            .ok()?;

        let listen_message = NlmsghdrBuilder::default()
            .nl_type(Nlmsg::Done)
            .nl_flags(NlmF::empty())
            .nl_pid(socket.pid())
            .nl_payload(NlPayload::Payload(listen_payload))
            .build()
            .inspect_err(|e| error!("failed to build listen message: {e}"))
            .ok()?;

        if let Err(e) = socket.send(&listen_message).await {
            error!("failed to send listen message: {e}");
            return None;
        }

        let deadline = tokio::time::Instant::now() + LISTEN_ACK_TIMEOUT;

        loop {
            let recv = socket.recv::<Nlmsg, CnMsg<ProcEventHeader>>();
            let messages = match tokio::time::timeout_at(deadline, recv).await {
                Ok(Ok((e, _))) => e,
                Ok(Err(SocketError::Io(e))) if e.raw_os_error() == Some(ENOBUFS) => continue,
                Ok(Err(e)) => {
                    error!("failed to read listen ack: {e}");
                    return None;
                }
                Err(_) => {
                    error!("listen ack not received within {LISTEN_ACK_TIMEOUT:?}");
                    return None;
                }
            };

            let Some(err) = messages.filter_map(Result::ok).find_map(|message| {
                let payload = message.get_payload()?;
                if *payload.ack() == response_listen_ack
                    && let ProcEvent::Ack { err } = payload.payload().event
                {
                    return Some(err);
                }
                None
            }) else {
                continue;
            };

            if err != 0 {
                let err = std::io::Error::from_raw_os_error(err as i32);
                error!("listen request rejected: {err}");
                return None;
            }
            return Some(socket);
        }
    }
}

/// Converts connector event timestamp_ns (`ClockId::Monotonic`, nanoseconds)
/// into `/proc/<pid>/stat` start_time (`ClockId::Boottime`, clock ticks)
fn monotonic_ns_to_boot_ticks(monotonic_ns: u64) -> u64 {
    static NS_TO_TICKS: OnceLock<u64> = OnceLock::new();

    fn timespec_to_ns(ts: Timespec) -> u64 {
        let s = ts.tv_sec as u64;
        let ns = ts.tv_nsec as u64;
        s.saturating_mul(1_000_000_000).saturating_add(ns)
    }

    let ns_to_ticks = *NS_TO_TICKS.get_or_init(|| 1_000_000_000 / procfs::ticks_per_second());
    let base_boot_time = clock_gettime(ClockId::Boottime);
    let base_monotonic = clock_gettime(ClockId::Monotonic);

    let time = monotonic_ns
        .saturating_add(timespec_to_ns(base_boot_time))
        .saturating_sub(timespec_to_ns(base_monotonic));

    time / ns_to_ticks
}
