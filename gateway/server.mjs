// gateway/server.mjs — TLS-terminating HTTP gateway for the entheai Worker's
// Railway Redis store.
//
// HTTPS terminates at Railway (auto-TLS on the public domain); this process
// only ever talks to Redis over the private `redis.railway.internal` network,
// so REDISPASSWORD never transits any public link. The only credential that
// crosses public network is GATEWAY_TOKEN, and it travels inside TLS.
//
// Zero runtime deps: Redis access is a minimal RESP2 client over node:net
// (same wire format the Worker's src/redis.mjs speaks); HTTP is node:http.
//
// Env:
//   REDIS_HOST        default "redis.railway.internal"
//   REDIS_PORT        default 6379
//   REDISUSER         default "default"
//   REDISPASSWORD     required (Railway private network still requires AUTH)
//   GATEWAY_TOKEN     required — every request needs `Authorization: Bearer <token>`
//   PORT              default 8080 (Railway injects PORT)

import http from "node:http";
import net from "node:net";
import { pathToFileURL } from "node:url";
import { createHandler } from "./handler.mjs";

// ---- minimal RESP2 client over node:net (one socket per command) ----------

function encodeCommand(...parts) {
  let out = `*${parts.length}\r\n`;
  for (const p of parts) {
    const s = String(p);
    out += `$${Buffer.byteLength(s, "utf8")}\r\n${s}\r\n`;
  }
  return Buffer.from(out, "utf8");
}

// createReader(socket) -> async () => reply. Handles the RESP2 reply types the
// gateway needs: +simple, -error (throws), :integer, $bulk ($-1 = null).
function createReader(socket) {
  let buf = Buffer.alloc(0);

  const nextChunk = () =>
    new Promise((resolve, reject) => {
      const onData = (chunk) => {
        cleanup();
        buf = Buffer.concat([buf, chunk]);
        resolve();
      };
      const onError = (e) => {
        cleanup();
        reject(e);
      };
      const onEnd = () => {
        cleanup();
        reject(new Error("redis: socket closed mid-reply"));
      };
      const cleanup = () => {
        socket.off("data", onData);
        socket.off("error", onError);
        socket.off("end", onEnd);
      };
      socket.once("data", onData);
      socket.once("error", onError);
      socket.once("end", onEnd);
    });

  async function readReply() {
    const line = async () => {
      for (;;) {
        const i = buf.indexOf("\r\n");
        if (i >= 0) {
          const s = buf.subarray(0, i).toString("utf8");
          buf = buf.subarray(i + 2);
          return s;
        }
        await nextChunk();
      }
    };

    const l = await line();
    const t = l[0];
    const rest = l.slice(1);
    if (t === "+") return rest; // e.g. +OK, +PONG
    if (t === "-") throw new Error("redis: " + rest);
    if (t === ":") return Number(rest);
    if (t === "$") {
      const n = Number(rest);
      if (n === -1) return null; // nil bulk string (missing key)
      while (buf.length < n + 2) await nextChunk();
      const v = buf.subarray(0, n).toString("utf8");
      buf = buf.subarray(n + 2);
      return v;
    }
    throw new Error("redis: unhandled RESP type " + t);
  }

  return readReply;
}

// command(opts, ...parts) — one socket: connect -> (AUTH) -> command -> reply.
// Self-healing across Redis restarts (no persistent connection to babysit).
function command(opts, ...parts) {
  return new Promise((resolve, reject) => {
    const socket = net.connect({ host: opts.host, port: opts.port });
    let finished = false;
    const fail = (e) => {
      if (!finished) {
        finished = true;
        reject(e);
      }
      socket.destroy();
    };
    const succeed = (v) => {
      if (!finished) {
        finished = true;
        resolve(v);
      }
      socket.end();
    };
    socket.on("error", fail);
    socket.on("connect", async () => {
      try {
        const readReply = createReader(socket);
        if (opts.password) {
          socket.write(encodeCommand("AUTH", opts.user || "default", opts.password));
          await readReply(); // +OK, throws on wrong password
        }
        socket.write(encodeCommand(...parts));
        succeed(await readReply());
      } catch (e) {
        fail(e);
      }
    });
  });
}

// createRedisStore({ host, port, user, password }) -> { get, set, del, ping }.
export function createRedisStore({ host, port, user, password } = {}) {
  const opts = { host, port, user: user || "default", password };
  return {
    async get(key) {
      return command(opts, "GET", key);
    },
    async set(key, value) {
      return command(opts, "SET", key, value);
    },
    async del(key) {
      return command(opts, "DEL", key);
    },
    async ping() {
      return command(opts, "PING");
    },
  };
}

// ---- node:http wiring ------------------------------------------------------

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on("data", (c) => {
      chunks.push(c);
      size += c.length;
    });
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

// startServer({ store, token, port, host }) — injectable store makes the whole
// HTTP path testable against an in-memory double. Returns the listening server.
export function startServer({ store, token, port = 8080, host = "0.0.0.0" } = {}) {
  const handle = createHandler({ store, token });
  const server = http.createServer(async (req, res) => {
    try {
      const rawBody = await readBody(req);
      let pathname;
      try {
        pathname = new URL(req.url ?? "/", "http://gateway.internal").pathname;
      } catch {
        pathname = req.url ?? "/";
      }
      const out = await handle(req.method, pathname, req.headers, rawBody);
      res.writeHead(out.status, out.headers);
      res.end(out.body);
    } catch (e) {
      console.error("gateway: request failed:", e);
      res.writeHead(500, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "internal error" }));
    }
  });
  server.listen(port, host);
  return server;
}

// ---- direct-run bootstrap --------------------------------------------------

const isMain =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;

if (isMain) {
  const token = process.env.GATEWAY_TOKEN;
  const password = process.env.REDISPASSWORD;
  if (!token) {
    console.error("gateway: GATEWAY_TOKEN is required");
    process.exit(1);
  }
  if (!password) {
    console.error("gateway: REDISPASSWORD is required");
    process.exit(1);
  }
  const store = createRedisStore({
    host: process.env.REDIS_HOST || "redis.railway.internal",
    port: Number(process.env.REDIS_PORT) || 6379,
    user: process.env.REDISUSER || "default",
    password,
  });
  const port = Number(process.env.PORT) || 8080;
  const server = startServer({ store, token, port });
  server.once("listening", () => {
    const redisHost = process.env.REDIS_HOST || "redis.railway.internal";
    const redisPort = Number(process.env.REDIS_PORT) || 6379;
    console.log(`redis gateway listening on :${port}, proxying ${redisHost}:${redisPort}`);
  });
}
