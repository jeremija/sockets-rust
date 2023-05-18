use std::{net::{SocketAddr, IpAddr}, env, sync::Arc};

use clap::{command, Parser};
use anyhow::Result;
use env_logger::Env;
use log::{info, error};
use sockets::{server::handle_stream, auth::{Authenticator, SingleKeyAuthenticator}};
use tokio::net::TcpListener;
use tokio_tungstenite::MaybeTlsStream;
use tokio_util::sync::CancellationToken;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "localhost")]
    hostname: String,

    #[arg(
        long,
        help = "Bind args for webserver socket",
        default_value = "127.0.0.1:3000"
    ) ]
    listen_addr: SocketAddr,

    #[arg(
        long,
        help = "Bind args for webserver socket",
        default_value = "0.0.0.0"
    )]
    adhoc_bind_addr: IpAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    let listener = TcpListener::bind(args.listen_addr).await?;

    info!("listening on {}", listener.local_addr()?);

    let api_key = env::var("SOCKETS_SINGLE_API_KEY").unwrap_or("".to_string());

    let authenticator: Arc<dyn Authenticator> = Arc::new(SingleKeyAuthenticator::new(api_key));

    loop {
        let (stream, addr) = listener.accept().await?;
        let hostname = args.hostname.clone();
        let adhoc_bind_addr = args.adhoc_bind_addr;

        info!("new TCP connection from: {} on {}", addr, args.listen_addr);

        let authenticator = authenticator.clone();
        let cancel = CancellationToken::new();

        tokio::spawn(async move {
            let cancel2 = cancel.clone();
            // TODO add TLS support.
            let stream = MaybeTlsStream::Plain(stream);

            match handle_stream(cancel2, authenticator, stream, addr, adhoc_bind_addr, hostname).await {
                Ok(()) => info!("stream done: {}", addr),
                Err(err) => error!("handling stream: {}: {:?}", addr, err),
            }

            cancel.cancel();
        });
    }
}
