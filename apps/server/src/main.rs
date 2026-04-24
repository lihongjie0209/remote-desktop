use clap::Parser;
use tracing_subscriber::{fmt, EnvFilter};

mod auth;
mod service;

#[derive(Parser)]
#[command(name = "remote-desktop-server", about = "Unified gRPC relay server")]
pub struct Args {
    /// Bind address
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    /// gRPC port
    #[arg(long, default_value_t = 50055)]
    pub port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("server=info".parse()?))
        .init();

    let args = Args::parse();
    service::run(args).await
}
