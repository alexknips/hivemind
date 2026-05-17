use tracing::info;
use tracing_subscriber::{fmt, EnvFilter};

use hivemind::cli;

fn main() {
    let cli_args = cli::parse();
    init_tracing(cli_args.verbose);
    info!(target: "hivemind", actor = %cli_args.actor, "hivemind starting");

    match cli::run(&cli_args) {
        Ok(output) => {
            println!("{output}");
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(cli::exit_code_for_error(&error).code());
        }
    }
}

fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => "hivemind=info",
        1 => "hivemind=debug",
        _ => "hivemind=trace",
    };

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .compact()
        .init();
}
