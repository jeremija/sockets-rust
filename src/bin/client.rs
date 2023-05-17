use std::net::SocketAddr;

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;

use clap::Parser;

use anyhow::Result;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Name of the person to greet
    #[arg(short, long, default_value = "ws://localhost:3000/socket")]
    server_url: Url,

    #[arg(short, long, help = "Local socket to expose, for example 127.0.0.1:22")]
    expose: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let (ws_stream, response) = connect_async(args.server_url).await?;

    println!("Connected to the server");
    println!("Response HTTP code: {}", response.status());
    println!("Response contains the following headers:");
    for (ref header, _value) in response.headers() {
        println!("* {}", header);
    }

    let (mut write, read) = ws_stream.split();
    write.send(Message::Text("HELLO".to_string())).await?;

    read.for_each(|msg| async {
        println!("received: {}", msg.unwrap());
    })
    .await;

    Ok(())
}
