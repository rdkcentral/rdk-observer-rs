//! Executable entry point for RDK Observer.
//!
//! Keep worker orchestration in the library's `runtime` module. Argument parsing,
//! configuration loading, and logging initialization will be added here before
//! handing control to the runtime once those interfaces are defined.

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use rdk_observer_core::runtime::RuntimeConfig;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_arguments();

    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_level(true)
                .with_span_events(FmtSpan::CLOSE),
        )
        .init();

    let config = RuntimeConfig {
        include_pids: args.pid.into_iter().collect(),
        include_commands: args.command.into_iter().collect(),
        include_rdk_processes: args.rdk_processes,
        exclude_kernel_threads: args.no_kernel_threads,
        exclude_userspace_processes: args.no_userspace_processes,
        ..Default::default()
    };

    rdk_observer_core::runtime::run(config).await
}

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
