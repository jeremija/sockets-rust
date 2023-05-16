use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let url = Url::parse("ws://localhost:3000/socket").unwrap();
    let (mut ws_stream, response) = connect_async(url).await?;

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
