#![forbid(unsafe_code)]

use clap::{Parser, ValueEnum};

#[derive(Clone, ValueEnum)]
enum Command {
    Update,
    Check,
}

#[derive(Parser)]
struct Arguments {
    command: Command,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    let command = match arguments.command {
        Command::Update => "update",
        Command::Check => "check",
    };
    frinkworks::run_catalogue_utility(command).await
}
