//! Executable entry point for RDK Observer.
//!
//! Keep worker orchestration in the library's `runtime` module. Argument parsing,
//! configuration loading, and logging initialization will be added here before
//! handing control to the runtime once those interfaces are defined.

#[cfg(feature = "tui")]
mod tui;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use rdk_observer_core::process_registry::ProcessRegistry;
use tokio::task::JoinSet;
use tracing_subscriber::EnvFilter;
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
    // TODO(#9): pass arguments to `runtime::run()` when implemented.
    let _args = parse_arguments();

    init_logger();

    let (process_registry, registry_writer) = ProcessRegistry::new();
    let mut join_set = JoinSet::<anyhow::Result<()>>::new();

    #[cfg(feature = "tui")]
    join_set.spawn(tui::run(process_registry.clone()));

    join_set.spawn(async move {
        rdk_observer_core::runtime::run(process_registry, registry_writer).await;
        Ok(())
    });

    if let Some(result) = join_set.join_next().await {
        result??;
    }

    Ok(())
}

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
