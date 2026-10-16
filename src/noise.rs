//! The secure channel: a Noise IK handshake carried in WebSocket binary frames.
//!
//! Noise IK means the connecting bot already knows the server's public key (it is in
//! this repository) and sends its own static key encrypted inside the first message.
//! Both sides prove they hold their private keys during the handshake; after it every
//! message is encrypted and authenticated with ChaCha20-Poly1305, and a replayed,
//! reordered or altered message fails to decrypt.

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use snow::{Builder, TransportState, params::NoiseParams};
use std::path::Path;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};

pub const PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
/// Mixed into the handshake, so a peer speaking another protocol fails at once.
pub const PROLOGUE: &[u8] = b"bot2bot/1";
/// Largest Noise message (the Noise specification's limit).
const MAX_NOISE: usize = 65_535;
/// Poly1305 tag added to every encrypted message.
const TAG: usize = 16;
/// Longest text one message may carry, in characters.
pub const MAX_TEXT: usize = 4_000;

/// What the bots say to each other, as JSON inside each encrypted message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    /// First message from the connecting bot (sent inside the handshake).
    Hello {
        name: String,
    },
    /// The server's handshake answer when it accepts the session.
    Welcome {
        name: String,
        note: String,
    },
    Text {
        text: String,
    },
    /// Liveness check; answered with `Pong`, never sent to a model.
    Ping,
    Pong,
    /// Either side ends the session; also how the server refuses one.
    Bye {
        reason: String,
    },
}

pub struct Keypair {
    pub private: Vec<u8>,
    pub public: Vec<u8>,
}

fn params() -> NoiseParams {
    PATTERN.parse().expect("valid Noise pattern")
}

impl Keypair {
    pub fn generate() -> Result<Self> {
        let k = Builder::new(params()).generate_keypair()?;
        Ok(Self {
            private: k.private,
            public: k.public,
        })
    }

    /// Key file format: `private <base64>` and `public <base64>`, one per line.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read key file {}", path.display()))?;
        let field = |name: &str| -> Result<Vec<u8>> {
            let line = text
                .lines()
                .find_map(|l| l.strip_prefix(name)?.strip_prefix(' '))
                .with_context(|| format!("{}: no `{name}` line", path.display()))?;
            decode_key(line)
        };
        Ok(Self {
            private: field("private")?,
            public: field("public")?,
        })
    }

    /// Write a new key file readable only by its owner; refuses to overwrite one.
    pub fn save_new(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts
            .open(path)
            .with_context(|| format!("create {} (it must not exist yet)", path.display()))?;
        writeln!(f, "private {}", encode_key(&self.private))?;
        writeln!(f, "public {}", encode_key(&self.public))?;
        Ok(())
    }

    /// Load the key at `path`, creating it first if there is none.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            return Self::load(path);
        }
        let k = Self::generate()?;
        k.save_new(path)?;
        Ok(k)
    }
}

pub fn encode_key(k: &[u8]) -> String {
    B64.encode(k)
}

pub fn decode_key(s: &str) -> Result<Vec<u8>> {
    let k = B64.decode(s.trim()).context("key is not base64")?;
    ensure!(k.len() == 32, "a key is 32 bytes, this one is {}", k.len());
    Ok(k)
}

/// WebSocket limits: one frame holds at most one Noise message.
pub fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_NOISE))
        .max_frame_size(Some(MAX_NOISE))
}

/// An established session: every `send` and `recv` is encrypted and authenticated.
pub struct Channel<S> {
    ws: WebSocketStream<S>,
    noise: TransportState,
    /// The other bot's static public key, proven during the handshake.
    pub peer: Vec<u8>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Channel<S> {
    pub async fn send(&mut self, m: &Msg) -> Result<()> {
        let frame = seal(&mut |p, b| Ok(self.noise.write_message(p, b)?), m)?;
        self.ws.send(Message::binary(frame)).await?;
        Ok(())
    }

    /// The next message, or `None` when the other side closed the connection.
    pub async fn recv(&mut self) -> Result<Option<Msg>> {
        let Some(frame) = next_binary(&mut self.ws).await? else {
            return Ok(None);
        };
        let mut buf = vec![0; frame.len()];
        let n = self
            .noise
            .read_message(&frame, &mut buf)
            .context("message failed authentication")?;
        Ok(Some(
            serde_json::from_slice(&buf[..n]).context("not a bot2bot message")?,
        ))
    }

    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }
}

/// Encrypts a payload into a buffer: a handshake or transport state's `write_message`.
type Writer<'a> = dyn FnMut(&[u8], &mut [u8]) -> Result<usize> + 'a;

/// Serialise `m` and encrypt it with `write`.
fn seal(write: &mut Writer, m: &Msg) -> Result<Vec<u8>> {
    let plain = serde_json::to_vec(m)?;
    // The handshake's first message also carries two keys (96 bytes with their tags).
    ensure!(
        plain.len() + TAG + 96 <= MAX_NOISE,
        "message too large to send"
    );
    let mut buf = vec![0; MAX_NOISE];
    let n = write(&plain, &mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

async fn next_binary<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
) -> Result<Option<Vec<u8>>> {
    while let Some(frame) = ws.next().await {
        match frame? {
            Message::Binary(b) => return Ok(Some(b.to_vec())),
            Message::Close(_) => return Ok(None),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => bail!("unexpected WebSocket frame: {other:?}"),
        }
    }
    Ok(None)
}

/// Connecting side. Fails unless the server proves it holds the private half of
/// `server_public`. Returns the channel and the server's answer (`Welcome` or `Bye`).
pub async fn initiate<S: AsyncRead + AsyncWrite + Unpin>(
    mut ws: WebSocketStream<S>,
    me: &Keypair,
    server_public: &[u8],
    hello: &Msg,
) -> Result<(Channel<S>, Msg)> {
    let mut hs = Builder::new(params())
        .local_private_key(&me.private)?
        .remote_public_key(server_public)?
        .prologue(PROLOGUE)?
        .build_initiator()?;
    let first = seal(&mut |p, b| Ok(hs.write_message(p, b)?), hello)?;
    ws.send(Message::binary(first)).await?;
    // A server without the published private key cannot read our first message, so it
    // drops the connection here instead of answering.
    let reply = next_binary(&mut ws)
        .await
        .ok()
        .flatten()
        .context("server did not complete the handshake: check the server key in server.txt")?;
    let mut buf = vec![0; reply.len()];
    let n = hs
        .read_message(&reply, &mut buf)
        .context("server did not prove it holds the published key")?;
    let answer: Msg = serde_json::from_slice(&buf[..n]).context("not a bot2bot message")?;
    let noise = hs.into_transport_mode()?;
    let ch = Channel {
        ws,
        noise,
        peer: server_public.to_vec(),
    };
    Ok((ch, answer))
}

/// Accepting side. `admit` sees the client's proven key and its `Hello`, and returns
/// the answer sent back inside the handshake: `Welcome` to go on, `Bye` to refuse.
pub async fn respond<S: AsyncRead + AsyncWrite + Unpin>(
    mut ws: WebSocketStream<S>,
    me: &Keypair,
    admit: impl FnOnce(&[u8], &Msg) -> Msg,
) -> Result<(Channel<S>, Msg, Msg)> {
    let mut hs = Builder::new(params())
        .local_private_key(&me.private)?
        .prologue(PROLOGUE)?
        .build_responder()?;
    let first = next_binary(&mut ws)
        .await?
        .context("client closed the connection before the handshake")?;
    let mut buf = vec![0; first.len()];
    let n = hs
        .read_message(&first, &mut buf)
        .context("handshake failed: wrong server key or not a bot2bot client")?;
    let hello: Msg = serde_json::from_slice(&buf[..n]).context("not a bot2bot message")?;
    let peer = hs
        .get_remote_static()
        .context("client sent no static key")?
        .to_vec();
    let answer = admit(&peer, &hello);
    let second = seal(&mut |p, b| Ok(hs.write_message(p, b)?), &answer)?;
    ws.send(Message::binary(second)).await?;
    let noise = hs.into_transport_mode()?;
    Ok((Channel { ws, noise, peer }, hello, answer))
}
