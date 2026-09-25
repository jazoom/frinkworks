#![forbid(unsafe_code)]

use clap::Parser;

#[derive(Parser)]
struct Arguments {
    /// Set the default log level. RUST_LOG directives take precedence.
    #[arg(long, default_value = "info")]
    log_level: tracing::Level,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    frinkworks::run_server(arguments.log_level).await
}

#[cfg(test)]
#[path = "main/tests.rs"]
mod tests;
