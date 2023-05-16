use std::{net::SocketAddr, str::FromStr, convert::Infallible};

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, Method, StatusCode};
use sockets::error::{Error, Result};

use futures::stream::TryStreamExt;

#[tokio::main]
async fn main() -> Result<()> {
    let addr = SocketAddr::from_str("127.0.0.1:3000").map_err(Error::AddrParseError)?;

    let make_svc = make_service_fn(|_conn| async {
        Ok::<_, Infallible>(service_fn(hello_world))
    });

    let server = Server::bind(&addr).serve(make_svc);

    let graceful = server.with_graceful_shutdown(shutdown_signal());

    eprintln!("Bound at: {}", addr);

    if let Err(e) = graceful.await {
        eprintln!("Server error: {}", e);
    }

    eprintln!("Bye!");

    Ok(())
}

async fn shutdown_signal() {
    // Wait for the CTRL+C signal
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");

    eprintln!("Graceful shutdown started");
}

async fn hello_world(req: Request<Body>) -> core::result::Result<Response<Body>, Infallible> {
    let mut response = Response::new(Body::empty());

    match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => {
            *response.body_mut() = Body::from("Try POSTing data to /echo");
        },
        (&Method::POST, "/echo") => {
            let mapping = req
                .into_body()
                .map_ok(|chunk| {
                    chunk.iter()
                        .map(|byte| byte.to_ascii_uppercase())
                        .collect::<Vec<u8>>()
                });

            *response.body_mut() = Body::wrap_stream(mapping);
        },
        _ => {
            *response.status_mut() = StatusCode::NOT_FOUND;
        },
    };

    Ok(response)
}
