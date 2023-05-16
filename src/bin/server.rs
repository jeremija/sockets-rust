use std::{net::SocketAddr, str::FromStr};

use anyhow::Result;
use futures::{SinkExt, StreamExt};
use hyper::service::service_fn;
use hyper::{Body, Request, Response};
use hyper_tungstenite::tungstenite::Message;

use hyper_tungstenite::HyperWebsocket;

#[tokio::main]
async fn main() -> Result<()> {
    let addr = SocketAddr::from_str("127.0.0.1:3000")?;

    // let make_svc = make_service_fn(|conn| async {
    //     Ok::<_, Infallible>(service_fn(hello_world))
    // });

    // let server = Server::bind(&addr)
    //     .serve(make_svc)
    //     .with_graceful_shutdown(shutdown_signal());
    //
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("listening on {}", addr);

    let mut http = hyper::server::conn::Http::new();
    http.http1_only(true);
    http.http1_keep_alive(true);

    loop {
        let (stream, _) = listener.accept().await?;

        let connection = http
            .serve_connection(stream, service_fn(handle_request))
            .with_upgrades();

        tokio::spawn(async move {
            if let Err(err) = connection.await {
                println!("Error serving HTTP connection: {:?}", err);
            }
        });
    }
}

// async fn shutdown_signal() {
//     // Wait for the CTRL+C signal
//     tokio::signal::ctrl_c()
//         .await
//         .expect("failed to install CTRL+C signal handler");

//     eprintln!("Graceful shutdown started");
// }

// async fn hello_world(req: Request<Body>) -> core::result::Result<Response<Body>, Infallible> {
//     let mut response = Response::new(Body::empty());

//     match (req.method(), req.uri().path()) {
//         (&Method::GET, "/") => {
//             *response.body_mut() = Body::from("Try POSTing data to /echo");
//         },
//         (&Method::POST, "/echo") => {
//             let mapping = req
//                 .into_body()
//                 .map_ok(|chunk| {
//                     chunk.iter()
//                         .map(|byte| byte.to_ascii_uppercase())
//                         .collect::<Vec<u8>>()
//                 });

//             *response.body_mut() = Body::wrap_stream(mapping);
//         },
//         _ => {
//             *response.status_mut() = StatusCode::NOT_FOUND;
//         },
//     };

//     Ok(response)
// }

/// Handle a HTTP or WebSocket request.
async fn handle_request(mut request: Request<Body>) -> Result<Response<Body>> {
    // Check if the request is a websocket upgrade request.
    if hyper_tungstenite::is_upgrade_request(&request) {
        let (response, websocket) = hyper_tungstenite::upgrade(&mut request, None)?;

        // Spawn a task to handle the websocket connection.
        tokio::spawn(async move {
            if let Err(e) = serve_websocket(websocket).await {
                eprintln!("Error in websocket connection: {}", e);
            }
        });

        // Return the response so the spawned future can continue.
        Ok(response)
    } else {
        // Handle regular HTTP requests here.
        Ok(Response::new(Body::from("Hello HTTP!")))
    }
}

/// Handle a websocket connection.
async fn serve_websocket(websocket: HyperWebsocket) -> Result<()> {
    let mut websocket = websocket.await?;

    while let Some(message) = websocket.next().await {
        match message? {
            Message::Text(msg) => {
                println!("Received text message: {}", msg);
                websocket
                    .send(Message::text("Thank you, come again."))
                    .await?;
            }
            Message::Binary(msg) => {
                println!("Received binary message: {:02X?}", msg);
                websocket
                    .send(Message::binary(b"Thank you, come again.".to_vec()))
                    .await?;
            }
            Message::Ping(msg) => {
                // No need to send a reply: tungstenite takes care of this for you.
                println!("Received ping message: {:02X?}", msg);
            }
            Message::Pong(msg) => {
                println!("Received pong message: {:02X?}", msg);
            }
            Message::Close(msg) => {
                // No need to send a reply: tungstenite takes care of this for you.
                if let Some(msg) = &msg {
                    println!(
                        "Received close message with code {} and message: {}",
                        msg.code, msg.reason
                    );
                } else {
                    println!("Received close message");
                }
            }
            Message::Frame(_msg) => {
                unreachable!();
            }
        }
    }

    Ok(())
}
