use std::{net::{SocketAddr, IpAddr}, env, sync::Arc, path::PathBuf};
use clap::{command, Parser};
use anyhow::Result;
use env_logger::Env;
use log::{info, error};
use sockets::{server::handle_stream, auth::{Authenticator, SingleKeyAuthenticator}, tls::{load_certs, load_keys, MaybeTlsStream}, error::Error};
use tokio::net::TcpListener;
use tokio_rustls::{rustls, TlsAcceptor};
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
        help = "Bind address for sockets created after expose calls",
        default_value = "0.0.0.0"
    )]
    adhoc_bind_addr: IpAddr,

    #[arg(long, help = "TLS certificate file. Both cert and cert key need to be set for TLS to be enabled.")]
    cert: Option<PathBuf>,

    #[arg(long, help = "TLS certificate key file. Both cert and cert key need to be set for TLS to be enabled.")]
    cert_key: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    let mut tls_acceptor = None;

    if let (Some(cert), Some(cert_key)) = (args.cert, args.cert_key) {
        let certs = load_certs(&cert)?;
        let mut keys = load_keys(&cert_key)?;

        if certs.len() == 0 {
            return Err(Error::NoTlsCertificates.into());
        }

        if keys.len() == 0 {
            return Err(Error::NoTlsCertificateKeys.into());
        }

        let config = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(certs, keys.remove(0))?;

        tls_acceptor = Some(TlsAcceptor::from(Arc::new(config)));
    }

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

        let tls_acceptor = tls_acceptor.clone();

        tokio::spawn(async move {
            let maybe_tls_stream = match tls_acceptor {
                Some(tls_acceptor) => {
                    match tls_acceptor.accept(stream).await {
                        Ok(stream) => MaybeTlsStream::RustlsServer(stream),
                        Err(err) => {
                            error!("failed to accept TLS connection from {}: {:?}", addr, err);
                            return;
                        }
                    }
                }
                None => MaybeTlsStream::Plain(stream),
            };

            let cancel2 = cancel.clone();

            match handle_stream(cancel2, authenticator, maybe_tls_stream, addr, adhoc_bind_addr, hostname).await {
                Ok(()) => info!("stream done: {}", addr),
                Err(err) => error!("handling stream: {}: {:?}", addr, err),
            }

            cancel.cancel();
        });
    }
}
