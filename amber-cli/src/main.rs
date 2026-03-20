mod cli;
mod tui;

use anyhow::Result;
use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("amber=info")
        .init();
    let cli = Cli::parse();
    cli::run(cli).await
}
