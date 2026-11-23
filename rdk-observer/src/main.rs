//! Executable entry point for RDK Observer.
//!
//! Keep worker orchestration in the library's `runtime` module. Argument parsing,
//! configuration loading, and logging initialization will be added here before
//! handing control to the runtime once those interfaces are defined.

#[cfg(feature = "rbus")]
mod rbus;
#[cfg(feature = "tui")]
mod tui;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use rdk_observer_core::process_filter::ProcessFilter;
use rdk_observer_core::process_worker::ProcessId;
use rdk_observer_core::runtime::{Runtime, RuntimeConfig};
use std::num::NonZeroU64;
use std::time::Duration;
use tokio::task::JoinSet;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

////////////////////////////////////////////////////////////////////////////////
/// All processes are captured by default, the options below narrow that scope
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Include the process with this PID; repeat to add more
    #[arg(short, long, value_name = "PID")]
    pid: Vec<u32>,
    /// Include processes with this command name; repeat to add more
    #[arg(short, long, value_name = "COMM")]
    command: Vec<String>,
    /// Include RDK processes from the configured selection list
    #[arg(long)]
    rdk_processes: bool,
    /// Exclude kernel threads
    #[arg(long)]
    no_kernel_threads: bool,
    /// Exclude userspace processes
    #[arg(long)]
    no_userspace_processes: bool,
    /// Interval in seconds at which to collect system stats
    #[arg(long, value_name = "SECS", default_value_t = NonZeroU64::new(RuntimeConfig::default().system_collection_interval.as_secs()).unwrap())]
    system_collection_interval: NonZeroU64,
    /// Interval in seconds at which to collect each process stats for first few samples
    #[arg(long, value_name = "SECS", default_value_t = NonZeroU64::new(RuntimeConfig::default().process_collection_interval_initial.as_secs()).unwrap())]
    process_collection_interval_initial: NonZeroU64,
    /// Interval in seconds at which to collect each process stats for longer running processes
    #[arg(long, value_name = "SECS", default_value_t = NonZeroU64::new(RuntimeConfig::default().process_collection_interval_ongoing.as_secs()).unwrap())]
    process_collection_interval_ongoing: NonZeroU64,
}

////////////////////////////////////////////////////////////////////////////////
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logger();

    let args = parse_arguments();
    let runtime = Runtime::new(RuntimeConfig {
        process_filter: ProcessFilter {
            include_pids: args.pid.into_iter().map(ProcessId::from).collect(),
            include_commands: args.command.into_iter().collect(),
            include_rdk_processes: args.rdk_processes,
            exclude_kernel_threads: args.no_kernel_threads,
            exclude_userspace_processes: args.no_userspace_processes,
        },
        system_collection_interval: Duration::from_secs(args.system_collection_interval.get()),
        process_collection_interval_initial: Duration::from_secs(
            args.process_collection_interval_initial.get(),
        ),
        process_collection_interval_ongoing: Duration::from_secs(
            args.process_collection_interval_ongoing.get(),
        ),
    });

    #[cfg(feature = "rbus")]
    let _handle = rbus::RBusConnection::open(runtime.registry());

    let mut join_set = JoinSet::new();

    #[cfg(feature = "tui")]
    join_set.spawn(tui::run(runtime.registry()));
    join_set.spawn(runtime.run());

    if let Some(result) = join_set.join_next().await {
        result??;
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
fn init_logger() {
    let registry = tracing_subscriber::registry().with(EnvFilter::from_default_env());

    #[cfg(not(feature = "tui"))]
    let registry = {
        use tracing_subscriber::fmt::format::FmtSpan;

        registry.with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_level(true)
                .with_span_events(FmtSpan::CLOSE),
        )
    };

    registry.init();
}

////////////////////////////////////////////////////////////////////////////////
/// Validate arguments combination that clap cannot express declaratively
fn parse_arguments() -> Args {
    let args = Args::parse();

    // excluding both process kinds only makes sense when some inclusion is also present
    if args.no_kernel_threads
        && args.no_userspace_processes
        && args.pid.is_empty()
        && args.command.is_empty()
        && !args.rdk_processes
    {
        Args::command().error(
            ErrorKind::ArgumentConflict,
            "'--no-kernel-threads' and '--no-userspace-processes' together require at least one '--pid', '--command' or `rdk-processes`",
        ).exit();
    }
    args
}
