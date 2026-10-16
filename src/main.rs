mod brain;
mod client;
mod noise;
mod server;

use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::PathBuf;

const USAGE: &str = "\
bot2bot: an encrypted, mutually authenticated, real-time channel between two bots

  bot2bot keygen <file>                   create a key pair (file must not exist); prints the public key
  bot2bot chat [--url U] [--server-key K] [--key FILE] [--name N]
                                          talk to Fery's bot: one line in, one line out
  bot2bot serve [--listen ADDR] [--key FILE] [--data DIR] [--echo] [--allow FILE]
                                          run Fery's side (see README: Running the server)
  bot2bot selftest                        run both sides locally and check the channel

Defaults: --url and --server-key come from server.txt in this repository,
--key is ./client.key (created on first use), --name is \"claude-mem bot\".";

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// server.txt holds the published endpoint and the server's public key.
fn published(field: &str) -> Result<String> {
    let text = std::fs::read_to_string("server.txt").context(
        "server.txt not found: run from the repository root or pass --url and --server-key",
    )?;
    text.lines()
        .find_map(|l| l.strip_prefix(field)?.strip_prefix(' '))
        .map(|s| s.trim().to_string())
        .with_context(|| format!("server.txt has no `{field}` line"))
}

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => {
            let path = args.get(1).context("usage: bot2bot keygen <file>")?;
            let k = noise::Keypair::generate()?;
            k.save_new(path.as_ref())?;
            println!("{}", noise::encode_key(&k.public));
        }
        Some("chat") => {
            let url = arg(&args, "--url").map_or_else(|| published("endpoint"), Ok)?;
            let server = arg(&args, "--server-key").map_or_else(|| published("public_key"), Ok)?;
            let key = PathBuf::from(arg(&args, "--key").unwrap_or_else(|| "client.key".into()));
            let name = arg(&args, "--name").unwrap_or_else(|| "claude-mem bot".into());
            let me = noise::Keypair::load_or_create(&key)?;
            eprintln!("your key: {}", noise::encode_key(&me.public));
            let ch = client::connect(&url, &me, &noise::decode_key(&server)?, &name).await?;
            client::chat(ch).await?;
        }
        Some("serve") => serve(&args).await?,
        Some("selftest") => selftest().await?,
        _ => {
            println!("{USAGE}");
        }
    }
    Ok(())
}

async fn serve(args: &[String]) -> Result<()> {
    // Hosted platforms pass the port in PORT and secrets in the environment.
    let listen = arg(args, "--listen")
        .or_else(|| std::env::var("PORT").ok().map(|p| format!("0.0.0.0:{p}")))
        .unwrap_or_else(|| "127.0.0.1:8787".into());
    let data = PathBuf::from(arg(args, "--data").unwrap_or_else(|| "data".into()));
    let key = match (arg(args, "--key"), std::env::var("BOT2BOT_SERVER_KEY")) {
        (None, Ok(private)) => noise::Keypair::from_private(&private)
            .context("BOT2BOT_SERVER_KEY (the `private` value from a key file)")?,
        (path, _) => noise::Keypair::load(path.as_deref().unwrap_or("server.key").as_ref())
            .context("server key (create one with `bot2bot keygen`)")?,
    };
    let allow = match arg(args, "--allow") {
        None => None,
        Some(f) => Some(
            std::fs::read_to_string(&f)?
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(String::from)
                .collect::<HashSet<_>>(),
        ),
    };
    let brain: Box<dyn brain::Brain> = if args.iter().any(|a| a == "--echo") {
        Box::new(brain::Echo)
    } else {
        let env = |n: &str| std::env::var(n).with_context(|| format!("set {n}"));
        let profile = std::fs::read_to_string("profile/fery.md").context("profile/fery.md")?;
        Box::new(brain::OpenAiCompat::new(
            env("BOT2BOT_MODEL_URL")?,
            env("BOT2BOT_MODEL_KEY")?,
            &env("BOT2BOT_MODEL")?,
            &profile,
        ))
    };
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    eprintln!(
        "listening on {listen}, public key {}",
        noise::encode_key(&key.public)
    );
    let cfg = server::Config {
        key,
        name: "Fery's bot".into(),
        limits: server::Limits::default(),
        allow,
        data,
    };
    server::serve(listener, cfg, brain).await
}

/// Both sides in one process over a real socket: the happy path and every refusal a
/// reviewer would want to see.
async fn selftest() -> Result<()> {
    let server_key = noise::Keypair::generate()?;
    let server_public = server_key.public.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://{}", listener.local_addr()?);
    let data = std::env::temp_dir().join(format!("bot2bot-selftest-{}", std::process::id()));
    let cfg = server::Config {
        key: server_key,
        name: "selftest".into(),
        limits: server::Limits::default(),
        allow: None,
        data: data.clone(),
    };
    tokio::spawn(server::serve(listener, cfg, Box::new(brain::Echo)));
    let me = noise::Keypair::generate()?;

    // 1. Handshake and an encrypted round trip.
    let mut ch = client::connect(&url, &me, &server_public, "t").await?;
    ch.send(&noise::Msg::Text {
        text: "hello".into(),
    })
    .await?;
    match ch.recv().await? {
        Some(noise::Msg::Text { text }) if text == "echo: hello" => {}
        other => bail!("round trip: {other:?}"),
    }
    ch.send(&noise::Msg::Ping).await?;
    if ch.recv().await? != Some(noise::Msg::Pong) {
        bail!("ping");
    }
    ch.close().await;
    println!("ok   handshake, encrypted round trip, ping");

    // 2. A server that does not hold the published key is rejected by the client.
    let wrong = noise::Keypair::generate()?.public;
    match client::connect(&url, &me, &wrong, "t").await {
        Err(e) => println!("ok   wrong server key refused: {e:#}"),
        Ok(_) => bail!("connected with the wrong server key"),
    }

    // 3. Plain ws:// to anywhere but localhost is refused.
    if client::connect("ws://example.com", &me, &server_public, "t")
        .await
        .is_ok()
    {
        bail!("plain ws accepted");
    }
    println!("ok   plain ws:// to a remote host refused");

    // 4. Over-long messages end the session.
    let mut ch = client::connect(&url, &me, &server_public, "t").await?;
    ch.send(&noise::Msg::Text {
        text: "x".repeat(noise::MAX_TEXT + 1),
    })
    .await?;
    match ch.recv().await? {
        Some(noise::Msg::Bye { reason }) => {
            println!("ok   over-long message ends the session: {reason}")
        }
        other => bail!("over-long message: {other:?}"),
    }

    // 5. The kill switch refuses new sessions.
    std::fs::write(data.join("paused"), "")?;
    match client::connect(&url, &me, &server_public, "t").await {
        Err(e) => println!("ok   paused server refuses: {e:#}"),
        Ok(_) => bail!("paused server accepted"),
    }
    std::fs::remove_dir_all(&data).ok();
    println!("selftest passed");
    Ok(())
}
