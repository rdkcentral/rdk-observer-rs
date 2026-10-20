//! Executable entry point for RDK Observer.
//!
//! Keep worker orchestration in the library's `runtime` module. Argument parsing,
//! configuration loading, and logging initialization will be added here before
//! handing control to the runtime once those interfaces are defined.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_level(true)
                .with_span_events(FmtSpan::CLOSE),
        )
        .init();

    rdk_observer_core::runtime::run().await;
}
