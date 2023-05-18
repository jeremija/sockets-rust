use std::{net::SocketAddr, env};

use anyhow::Result;
use env_logger::Env;
use sockets::client::dial_and_start;
use url::Url;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Name of the person to greet
    #[arg(long, default_value = "ws://localhost:3000")]
    server_url: Url,

    #[arg(short, long, help = "Local socket to expose, for example 127.0.0.1:22")]
    expose: Vec<SocketAddr>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let api_key = env::var("SOCKETS_API_KEY").unwrap_or("".to_string());

    let args = Args::parse();

    dial_and_start(
        api_key,
        args.server_url,
        args.expose,
    ).await?;

    Ok(())
}
