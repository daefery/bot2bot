//! The connecting side: what the other company's bot runs to talk to Fery's bot.

use crate::noise::{self, Channel, Keypair, Msg};
use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};

/// Connect, complete the handshake, and return the open channel. Fails unless the
/// server proves it holds the private half of `server_public` and welcomes us.
pub async fn connect(
    url: &str,
    me: &Keypair,
    server_public: &[u8],
    name: &str,
) -> Result<Channel<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>> {
    if !(url.starts_with("wss://")
        || url.starts_with("ws://127.0.0.1")
        || url.starts_with("ws://localhost"))
    {
        bail!("use a wss:// address (plain ws:// is only allowed to localhost)");
    }
    let (ws, _) = tokio_tungstenite::connect_async_with_config(url, Some(noise::ws_config()), true)
        .await
        .with_context(|| format!("connect to {url}"))?;
    let hello = Msg::Hello { name: name.into() };
    let (ch, answer) = noise::initiate(ws, me, server_public, &hello).await?;
    match answer {
        Msg::Welcome { name, note } => {
            eprintln!("connected to {name} (server key verified)\n{note}");
            Ok(ch)
        }
        Msg::Bye { reason } => bail!("server refused the session: {reason}"),
        other => bail!("unexpected handshake answer: {other:?}"),
    }
}

/// Line-based conversation: each line on stdin is one message, each reply is printed
/// as one line on stdout. Easy to drive from another program.
pub async fn chat<S: AsyncRead + AsyncWrite + Unpin>(mut ch: Channel<S>) -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "/bye" {
            ch.send(&Msg::Bye {
                reason: "done".into(),
            })
            .await?;
            break;
        }
        ch.send(&Msg::Text { text: line.into() }).await?;
        match ch.recv().await? {
            Some(Msg::Text { text }) => println!("{}", text.replace('\n', " ")),
            Some(Msg::Bye { reason }) => {
                eprintln!("server ended the session: {reason}");
                break;
            }
            Some(other) => eprintln!("unexpected: {other:?}"),
            None => {
                eprintln!("server closed the connection");
                break;
            }
        }
    }
    ch.close().await;
    Ok(())
}
