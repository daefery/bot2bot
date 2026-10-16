// One bot2bot session over a WebSocket: the handshake, then encrypted JSON messages.
// Shared by the Durable Object (production) and the Node test server (interop test).

import { b64, CipherState, respond, type Keypair } from "./noise.ts";

export const MAX_TEXT = 4000;
const MAX_FRAME = 65535;

export type Msg =
  | { type: "hello"; name: string }
  | { type: "welcome"; name: string; note: string }
  | { type: "text"; text: string }
  | { type: "ping" }
  | { type: "pong" }
  | { type: "bye"; reason: string };

const enc = new TextEncoder();
const dec = new TextDecoder();

/** What a session needs from its host: where to send frames, and what to say. */
export interface Host {
  send(frame: Uint8Array): void;
  close(): void;
  log(event: Record<string, unknown>): void;
  /** Decides on a new session: a welcome or a bye. */
  admit(peerB64: string, hello: Msg): Promise<Msg>;
  /** One reply to the conversation so far, or a bye that ends the session. */
  reply(history: [string, string][], peerB64: string): Promise<{ text: string } | { bye: string }>;
}

/** State that must survive between messages (and hibernation, as plain data). */
export interface State {
  phase: "handshake" | "open" | "closed";
  peer?: string;
  /** Transport keys and counters, so a woken Durable Object can carry on. */
  recv?: { k: string; n: string };
  send?: { k: string; n: string };
  history: [string, string][];
  last: number;
}

export const fresh = (): State => ({ phase: "handshake", history: [], last: 0 });

const restore = (s: { k: string; n: string }) => new CipherState(b64.decode(s.k), BigInt(s.n));

const save = (c: CipherState) => ({ k: b64.encode(c.k!), n: c.n.toString() });

export async function onFrame(
  st: State,
  frame: Uint8Array,
  me: Keypair,
  host: Host,
  gapMs = 500,
): Promise<void> {
  if (st.phase === "closed") return;
  if (frame.length > MAX_FRAME) return end(st, host, null, "frame too large");

  if (st.phase === "handshake") {
    let hs;
    try {
      hs = respond(me, frame);
    } catch {
      // Not encrypted to our key: drop the connection without answering.
      st.phase = "closed";
      host.log({ note: "handshake failed" });
      host.close();
      return;
    }
    const peer = b64.encode(hs.peer);
    let hello: Msg;
    try {
      hello = JSON.parse(dec.decode(hs.payload));
    } catch {
      hello = { type: "bye", reason: "bad hello" };
    }
    const answer = await host.admit(peer, hello);
    const { message, transport } = hs.answer(enc.encode(JSON.stringify(answer)));
    host.log({ peer, dir: "in", msg: hello });
    host.log({ peer, dir: "out", msg: answer });
    host.send(message);
    if (answer.type !== "welcome") {
      st.phase = "closed";
      host.close();
      return;
    }
    st.phase = "open";
    st.peer = peer;
    st.recv = save(transport.recv);
    st.send = save(transport.send);
    return;
  }

  const recv = restore(st.recv!);
  const send = restore(st.send!);
  let msg: Msg;
  try {
    msg = JSON.parse(dec.decode(recv.decrypt(new Uint8Array(0), frame)));
  } catch {
    // Failed authentication: nonces are out of step, nothing after this can be trusted.
    st.phase = "closed";
    host.log({ peer: st.peer, note: "message failed authentication" });
    host.close();
    return;
  }
  st.recv = save(recv);
  host.log({ peer: st.peer, dir: "in", msg });

  const out = (m: Msg) => {
    host.log({ peer: st.peer, dir: "out", msg: m });
    host.send(send.encrypt(new Uint8Array(0), enc.encode(JSON.stringify(m))));
    st.send = save(send);
  };

  switch (msg.type) {
    case "ping":
      return out({ type: "pong" });
    case "bye":
      st.phase = "closed";
      return host.close();
    case "text":
      break;
    default:
      return end(st, host, out, "unexpected message type");
  }
  if (typeof msg.text !== "string" || [...msg.text].length > MAX_TEXT)
    return end(st, host, out, "message too long");
  const now = Date.now();
  if (now - st.last < gapMs) return end(st, host, out, "messages too fast");
  st.last = now;

  st.history.push(["user", msg.text]);
  const r = await host.reply(st.history, st.peer!);
  if ("bye" in r) return end(st, host, out, r.bye);
  st.history.push(["assistant", r.text]);
  out({ type: "text", text: r.text });
}

function end(st: State, host: Host, out: ((m: Msg) => void) | null, reason: string) {
  if (out) out({ type: "bye", reason });
  st.phase = "closed";
  host.close();
}
