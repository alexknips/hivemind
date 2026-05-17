use tracing::info;
use tracing_subscriber::{fmt, EnvFilter};

fn main() {
    init_tracing();
    info!(target: "hivemind", "hivemind starting");
}

fn init_tracing() {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("hivemind=info"));

    fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .compact()
        .init();
}
