// Interop test server: the Worker's session code under Node, with an echo brain, so
// the Rust client can prove it talks to the TypeScript Noise implementation.
// Usage: node --experimental-strip-types test/server.mjs <port> <private-key-b64>
import { createServer } from "node:http";
import { WebSocketServer } from "ws";
import { b64, keypairFromPrivate } from "../src/noise.ts";
import { fresh, onFrame } from "../src/session.ts";

const [port, priv] = process.argv.slice(2);
const me = keypairFromPrivate(b64.decode(priv));
const server = createServer();
const wss = new WebSocketServer({ server });

wss.on("connection", (ws) => {
  const st = fresh();
  let chain = Promise.resolve();
  const host = {
    send: (f) => ws.send(f),
    close: () => ws.close(),
    log: () => {},
    admit: async (_peer, hello) =>
      hello.type === "hello"
        ? { type: "welcome", name: "ts-test", note: "ok" }
        : { type: "bye", reason: "the first message must be hello" },
    reply: async (history) => ({ text: "echo: " + history.at(-1)[1] }),
  };
  ws.on("message", (data) => {
    chain = chain.then(() => onFrame(st, new Uint8Array(data), me, host, 0));
  });
});
server.listen(Number(port), "127.0.0.1", () => console.log("ready"));
