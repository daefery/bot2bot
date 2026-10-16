//! The listening side: admits bots, enforces limits, keeps a transcript, and asks the
//! brain for each reply.

use crate::brain::Brain;
use crate::noise::{self, Keypair, MAX_TEXT, Msg};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::timeout;

pub struct Limits {
    /// Sessions open at the same time.
    pub sessions: usize,
    /// Replies per session.
    pub turns: usize,
    /// Model replies per UTC day, all bots together.
    pub daily: usize,
    /// Model replies per UTC day for one bot key.
    pub daily_per_key: usize,
    /// A session with no message for this long is closed.
    pub idle: Duration,
    /// Shortest gap between two messages in one session.
    pub gap: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            sessions: 4,
            turns: 40,
            daily: 300,
            daily_per_key: 100,
            idle: Duration::from_secs(600),
            gap: Duration::from_millis(500),
        }
    }
}

pub struct Config {
    pub key: Keypair,
    pub name: String,
    pub limits: Limits,
    /// When set, only these client keys (base64) are admitted.
    pub allow: Option<HashSet<String>>,
    /// Session transcripts (JSON lines) and the `paused` switch live here.
    pub data: PathBuf,
}

#[derive(Default)]
struct Usage {
    day: u64,
    total: usize,
    per_key: HashMap<String, usize>,
}

struct Shared {
    cfg: Config,
    brain: Box<dyn Brain>,
    usage: Mutex<Usage>,
    slots: Arc<Semaphore>,
}

fn today() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

impl Shared {
    /// The kill switch: a `paused` file in the data folder, or BOT2BOT_PAUSED=1 where
    /// the host has no shell (changing it on such hosts restarts the service).
    fn paused(&self) -> bool {
        self.cfg.data.join("paused").exists()
            || std::env::var("BOT2BOT_PAUSED").is_ok_and(|v| v == "1")
    }

    /// Counts one model reply against the daily budgets; false when one is spent.
    fn spend(&self, key: &str) -> bool {
        let mut u = self.usage.lock().unwrap();
        if u.day != today() {
            *u = Usage {
                day: today(),
                ..Usage::default()
            };
        }
        let mine = u.per_key.get(key).copied().unwrap_or(0);
        if u.total >= self.cfg.limits.daily || mine >= self.cfg.limits.daily_per_key {
            return false;
        }
        u.total += 1;
        *u.per_key.entry(key.to_string()).or_default() += 1;
        true
    }
}

pub async fn serve(listener: TcpListener, cfg: Config, brain: Box<dyn Brain>) -> Result<()> {
    std::fs::create_dir_all(cfg.data.join("sessions"))?;
    let shared = Arc::new(Shared {
        slots: Arc::new(Semaphore::new(cfg.limits.sessions)),
        cfg,
        brain,
        usage: Mutex::new(Usage::default()),
    });
    loop {
        let (tcp, addr) = listener.accept().await?;
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = session(tcp, &shared).await {
                eprintln!("{addr}: {e:#}");
            }
        });
    }
}

async fn session(tcp: TcpStream, sh: &Shared) -> Result<()> {
    let ws = timeout(
        Duration::from_secs(10),
        tokio_tungstenite::accept_async_with_config(tcp, Some(noise::ws_config())),
    )
    .await??;
    // A slot is taken before the handshake so a full server can say so inside it.
    let slot = sh.slots.clone().try_acquire_owned().ok();
    let admit = |peer: &[u8], hello: &Msg| -> Msg {
        let key = noise::encode_key(peer);
        let refuse = |reason: &str| Msg::Bye {
            reason: reason.into(),
        };
        if sh.paused() {
            return refuse("paused by the owner; try again later");
        }
        if !matches!(hello, Msg::Hello { .. }) {
            return refuse("the first message must be hello");
        }
        if let Some(allow) = &sh.cfg.allow
            && !allow.contains(&key)
        {
            return refuse("this key is not on the allow list");
        }
        if slot.is_none() {
            return refuse("busy: too many sessions open; try again in a minute");
        }
        Msg::Welcome {
            name: sh.cfg.name.clone(),
            note: format!(
                "Connected. Send {{\"type\":\"text\",\"text\":\"...\"}} messages (up to {MAX_TEXT} characters, {} replies per session). Ask about Fery's work, how this channel is built, or why ravnori is built the way it is. Send bye to end.",
                sh.cfg.limits.turns
            ),
        }
    };
    let (mut ch, hello, answer) = timeout(
        Duration::from_secs(10),
        noise::respond(ws, &sh.cfg.key, admit),
    )
    .await??;
    let key = noise::encode_key(&ch.peer);
    let mut log = Transcript::open(sh, &key)?;
    log.write("in", &hello);
    log.write("out", &answer);
    if matches!(answer, Msg::Bye { .. }) {
        ch.close().await;
        return Ok(());
    }

    let mut history: Vec<(String, String)> = Vec::new();
    let mut turns = 0;
    let mut last = Instant::now() - sh.cfg.limits.gap;
    loop {
        let msg = match timeout(sh.cfg.limits.idle, ch.recv()).await {
            Err(_) => {
                end(&mut ch, &mut log, "idle for too long").await;
                break;
            }
            Ok(Ok(None)) => break,
            Ok(Ok(Some(m))) => m,
            // A message that fails authentication ends the session: the channel's
            // nonces are out of step and nothing after it can be trusted.
            Ok(Err(e)) => {
                log.note(&format!("closed: {e:#}"));
                break;
            }
        };
        log.write("in", &msg);
        let text = match msg {
            Msg::Text { text } => text,
            Msg::Ping => {
                ch.send(&Msg::Pong).await?;
                continue;
            }
            Msg::Bye { .. } => break,
            _ => {
                end(&mut ch, &mut log, "unexpected message type").await;
                break;
            }
        };
        if text.chars().count() > MAX_TEXT {
            end(&mut ch, &mut log, "message too long").await;
            break;
        }
        if last.elapsed() < sh.cfg.limits.gap {
            end(&mut ch, &mut log, "messages too fast").await;
            break;
        }
        last = Instant::now();
        if turns >= sh.cfg.limits.turns {
            end(&mut ch, &mut log, "reply limit for this session reached").await;
            break;
        }
        if sh.paused() {
            end(&mut ch, &mut log, "paused by the owner").await;
            break;
        }
        if !sh.spend(&key) {
            end(&mut ch, &mut log, "daily reply budget spent; try tomorrow").await;
            break;
        }
        turns += 1;
        history.push(("user".into(), text));
        let reply = tokio::task::block_in_place(|| sh.brain.reply(&history));
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                log.note(&format!("brain: {e:#}"));
                end(
                    &mut ch,
                    &mut log,
                    "the model is unavailable; try again later",
                )
                .await;
                break;
            }
        };
        history.push(("assistant".into(), reply.clone()));
        let out = Msg::Text { text: reply };
        log.write("out", &out);
        ch.send(&out).await?;
    }
    ch.close().await;
    Ok(())
}

async fn end<S>(ch: &mut noise::Channel<S>, log: &mut Transcript, reason: &str)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let bye = Msg::Bye {
        reason: reason.into(),
    };
    log.write("out", &bye);
    let _ = ch.send(&bye).await;
}

/// One JSON-lines file per session under `data/sessions`, each line also printed to
/// stdout with the session tag: free hosting wipes the disk on restart, its log keeps it.
struct Transcript(std::fs::File, String);

impl Transcript {
    fn open(sh: &Shared, key: &str) -> Result<Self> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let tag: String = key
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(8)
            .collect();
        let path = sh
            .cfg
            .data
            .join("sessions")
            .join(format!("{now}-{tag}.jsonl"));
        let f = std::fs::File::create(path)?;
        let mut t = Self(f, format!("{now}-{tag}"));
        t.line(serde_json::json!({ "peer": key }));
        Ok(t)
    }

    fn line(&mut self, v: serde_json::Value) {
        let _ = writeln!(self.0, "{v}");
        println!("session {} {v}", self.1);
    }

    fn write(&mut self, dir: &str, m: &Msg) {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        self.line(serde_json::json!({ "at": at, "dir": dir, "msg": m }));
    }

    fn note(&mut self, s: &str) {
        self.line(serde_json::json!({ "note": s }));
    }
}
