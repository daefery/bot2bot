// Cloudflare Worker: routes each WebSocket to a Durable Object that runs the session.
// Durable Objects get 30 s of CPU per message on the free plan (plain Workers get 10 ms,
// too little for the X25519 work in JavaScript), and they can hibernate between
// messages without dropping the connection.

import { DurableObject } from "cloudflare:workers";
import profile from "../../profile/fery.md";
import { reply } from "./brain.ts";
import { b64, keypairFromPrivate } from "./noise.ts";
import { fresh, MAX_TEXT, onFrame, type Msg, type State } from "./session.ts";

export interface Env {
  SESSIONS: DurableObjectNamespace<Session>;
  BUDGET: DurableObjectNamespace<Budget>;
  BOT2BOT_SERVER_KEY: string;
  BOT2BOT_MODEL_KEY: string;
  BOT2BOT_MODEL_URL: string;
  BOT2BOT_MODEL: string;
  /** "1" refuses new sessions and ends open ones at their next message. */
  BOT2BOT_PAUSED?: string;
}

const LIMITS = { turns: 40, dailyPerKey: 100, daily: 300, idleMs: 10 * 60_000, gapMs: 500 };

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    if (req.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
      if (url.pathname === "/health") return new Response("ok\n");
      return new Response(
        "bot2bot endpoint. Connect with the client in https://github.com/daefery/bot2bot\n",
        { status: 426, headers: { Upgrade: "websocket" } },
      );
    }
    // One Durable Object per connection; another (Budget) keeps the daily counters.
    // A Durable Object is created near its first caller, and the Gemini API refuses some
    // locations ("User location is not supported"), so sessions are placed in the US.
    console.log(JSON.stringify({ note: "connect", colo: (req.cf as { colo?: string })?.colo }));
    const id = env.SESSIONS.newUniqueId();
    return env.SESSIONS.get(id, { locationHint: "wnam" }).fetch(req);
  },
};

export class Session extends DurableObject<Env> {
  async fetch(_req: Request): Promise<Response> {
    const [client, server] = Object.values(new WebSocketPair());
    server.binaryType = "arraybuffer";
    this.ctx.acceptWebSocket(server);
    server.serializeAttachment(fresh());
    await this.ctx.storage.setAlarm(Date.now() + LIMITS.idleMs);
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, data: ArrayBuffer | string) {
    if (typeof data === "string") return ws.close(1003, "binary frames only");
    const st = ws.deserializeAttachment() as State & { turns?: number };
    const env = this.env;
    const me = keypairFromPrivate(b64.decode(env.BOT2BOT_SERVER_KEY));
    const paused = env.BOT2BOT_PAUSED === "1";
    await this.ctx.storage.setAlarm(Date.now() + LIMITS.idleMs);

    await onFrame(
      st,
      new Uint8Array(data),
      me,
      {
        send: (f) => ws.send(f),
        close: () => ws.close(1000, "done"),
        log: (e) => console.log(JSON.stringify(e)),
        admit: async (_peer: string, hello: Msg): Promise<Msg> => {
          if (paused) return { type: "bye", reason: "paused by the owner; try again later" };
          if (hello.type !== "hello") return { type: "bye", reason: "the first message must be hello" };
          return {
            type: "welcome",
            name: "Fery's bot",
            note: `Connected. Send {"type":"text","text":"..."} messages (up to ${MAX_TEXT} characters, ${LIMITS.turns} replies per session). Ask about Fery's work, how this channel is built, or why ravnori is built the way it is. Send bye to end.`,
          };
        },
        reply: async (history, peer) => {
          if (paused) return { bye: "paused by the owner" };
          const turns = history.filter(([r]) => r === "user").length;
          if (turns > LIMITS.turns) return { bye: "reply limit for this session reached" };
          if (!(await spend(env, peer))) return { bye: "daily reply budget spent; try tomorrow" };
          try {
            return {
              text: await reply(
                {
                  url: env.BOT2BOT_MODEL_URL,
                  key: env.BOT2BOT_MODEL_KEY,
                  models: env.BOT2BOT_MODEL,
                  profile,
                },
                history,
              ),
            };
          } catch (e) {
            console.log(JSON.stringify({ peer, note: `brain: ${e}` }));
            return { bye: "the model is unavailable; try again later" };
          }
        },
      },
      LIMITS.gapMs,
    );
    if (st.phase !== "closed") ws.serializeAttachment(st);
  }

  /** No message for LIMITS.idleMs: end the session. */
  async alarm() {
    for (const ws of this.ctx.getWebSockets()) ws.close(1000, "idle for too long");
  }
}

/** Daily reply budgets, counted in one Durable Object so every session shares them. */
async function spend(env: Env, peer: string): Promise<boolean> {
  const budget = env.BUDGET.get(env.BUDGET.idFromName("daily"), { locationHint: "wnam" });
  return budget.spend(peer, LIMITS.daily, LIMITS.dailyPerKey);
}

export class Budget extends DurableObject<Env> {
  async spend(peer: string, daily: number, perKey: number): Promise<boolean> {
    const day = new Date().toISOString().slice(0, 10);
    const s = this.ctx.storage;
    const total = ((await s.get<number>(`${day}:total`)) ?? 0) + 1;
    const mine = ((await s.get<number>(`${day}:${peer}`)) ?? 0) + 1;
    if (total > daily || mine > perKey) return false;
    await s.put({ [`${day}:total`]: total, [`${day}:${peer}`]: mine });
    return true;
  }
}
