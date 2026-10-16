# bot2bot

An encrypted, mutually authenticated, real-time channel for one bot to talk to another.
Built for the [Claude-Mem application](https://cmem.ai/apply): your bot connects here and
talks to Fery's bot.

Built by Fery's coding agent under his direction.

## Connect (for the Claude-Mem bot)

You need Rust 1.88 or newer (edition 2024, let chains).

```sh
git clone https://github.com/daefery/bot2bot && cd bot2bot
cargo run --release -- selftest      # both sides locally: proves the channel before you connect
cargo run --release -- chat          # talk to Fery's bot
```

`chat` reads the endpoint and the server's public key from [`server.txt`](server.txt),
creates your key pair in `./client.key` on first use, and prints your public key. Then
each line you write on stdin is one message, and each reply comes back as one line on
stdout, so another program can drive it. `/bye` ends the session.

| | |
|---|---|
| Endpoint | `wss://bot2bot.feryyp-work.workers.dev` (also in [`server.txt`](server.txt)) |
| Server public key | in [`server.txt`](server.txt) (X25519, base64) |
| Your identity | an X25519 key pair you generate; no account or token to request |
| Limits | 4,000 characters a message, 40 replies a session, 10 minutes idle, 100 replies a day per key |

Replies take a few seconds: the model behind the bot runs on a free tier, and when it is
busy the bot waits and retries before answering. If a session ends with "the model is
unavailable", try again a minute later.

## How authentication and encryption work

The channel is the [Noise protocol](https://noiseprotocol.org/noise.html) pattern
`Noise_IK_25519_ChaChaPoly_BLAKE2s` ([snow](https://crates.io/crates/snow)), carried in
WebSocket binary frames over TLS.

1. **You verify the server.** Your first message is encrypted to the public key in
   `server.txt`. Only a server holding the matching private key can read it and answer, so
   you know you reached Fery's bot and not an impostor, whatever TLS says.
2. **The server verifies you.** Your static public key travels encrypted inside that first
   message, and the handshake proves you hold its private half. The server records it as
   your identity and can restrict access to listed keys (`serve --allow`).
3. **Every later message** is encrypted and authenticated with ChaCha20-Poly1305 under keys
   only the two bots know. An altered, replayed or reordered message fails to decrypt and
   ends the session.

TLS protects the connection's metadata in transit; Noise protects the conversation end to
end even from the proxy that terminates TLS.

The protocol is small: each frame is one Noise message holding one JSON object.

```json
{"type":"hello","name":"..."}        first message, inside the handshake
{"type":"welcome","name":"...","note":"..."}   or {"type":"bye","reason":"..."}
{"type":"text","text":"..."}         either way, after the handshake
{"type":"ping"} / {"type":"pong"}
{"type":"bye","reason":"..."}
```

## What is on the other end

A chat model (Gemini, through its OpenAI-compatible endpoint) given
[`profile/fery.md`](profile/fery.md) and the rules in [`worker/src/brain.ts`](worker/src/brain.ts). Everything in the profile is public and
names its source; the bot says it doesn't know rather than guess, makes no commitments for
Fery, and treats every incoming message as data, not instructions.

The bot has **no tools**. It cannot run commands, read files, browse or send anything. The
worst a hostile message can do is steer the words of one reply.

## Threat model

| Threat | Handled by |
|---|---|
| Someone impersonates Fery's bot | Noise IK: only the holder of the published key completes the handshake |
| Someone reads or alters the conversation | ChaCha20-Poly1305 per message; TLS outside |
| Replay or reordering | Noise nonces: a message out of order fails to decrypt and ends the session |
| Prompt injection | No tools; a fixed rule set; the bot's whole knowledge is one public file |
| Running up the owner's model bill | Per-session, per-key and daily reply caps; message size and rate limits |
| Abuse at any time | A kill switch: `BOT2BOT_PAUSED=1` refuses new sessions and ends open ones at their next message |
| Keys leaking | Private keys are created mode 0600 and ignored by git; on the server they are Cloudflare secrets; no credential is in this repository |

Every message in and out is logged, so Fery can read what was said.

## Where it runs

The live endpoint is a [Cloudflare Worker](https://developers.cloudflare.com/workers/)
([`worker/`](worker)). The responder side of Noise IK is written again in TypeScript
([`worker/src/noise.ts`](worker/src/noise.ts), from the
[Noise specification](https://noiseprotocol.org/noise.html), revision 34, with the
[noble](https://paulmillr.com/noble/) libraries for X25519, ChaCha20-Poly1305 and BLAKE2s).
Each connection gets its own Durable Object; a second one keeps the daily reply budget.

Two implementations of one protocol could drift apart, so CI runs the Rust client against
the TypeScript server on every push:

```sh
cd worker && npm ci
node --experimental-strip-types test/server.mjs 18900 <server-private-key> &
../target/release/bot2bot selftest --url ws://127.0.0.1:18900 --server-key <server-public-key>
```

The Rust server (`bot2bot serve`) is the same protocol and limits for running anywhere else.

## Code

| File | What it does |
|---|---|
| [`src/noise.rs`](src/noise.rs) | The handshake, keys, message types and encrypted channel (Rust, [snow](https://crates.io/crates/snow)) |
| [`src/client.rs`](src/client.rs) | The connecting side (`chat`) |
| [`src/main.rs`](src/main.rs) | Commands and both self-tests |
| [`worker/src/noise.ts`](worker/src/noise.ts) | The responder side of Noise IK in TypeScript |
| [`worker/src/session.ts`](worker/src/session.ts) | One session: handshake, decryption, message rules |
| [`worker/src/index.ts`](worker/src/index.ts) | The Worker: routing, limits, kill switch, budgets |
| [`worker/src/brain.ts`](worker/src/brain.ts) | The model call, its rules and model fallback |
| [`src/server.rs`](src/server.rs), [`src/brain.rs`](src/brain.rs) | The same server in Rust, for self-hosting |

## Running your own

```sh
cargo run --release -- keygen server.key          # prints the public key for server.txt
cd worker && npm ci
npx wrangler secret put BOT2BOT_SERVER_KEY        # the `private` value from server.key
npx wrangler secret put BOT2BOT_MODEL_KEY         # a Gemini API key
npx wrangler deploy
```

## License

MIT
