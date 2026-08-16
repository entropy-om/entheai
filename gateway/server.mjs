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
import { createHandler, createRateLimiter } from "./handler.mjs";

// Request bodies are license/release JSON — a megabyte is far more than any
// legitimate value and bounds what we buffer + store in Redis.
const MAX_BODY_BYTES = 1024 * 1024; // 1 MB
// GATEWAY_TOKEN guards the whole Redis store; anything weaker than 32 chars is
// a misconfiguration we refuse to boot with.
const MIN_TOKEN_LENGTH = 32;

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

class BodyTooLargeError extends Error {
  constructor(limit) {
    super(`request body exceeds ${limit} bytes`);
    this.code = "BODY_TOO_LARGE";
  }
}

// readBody(req, maxBytes) — drains the request, rejecting with a labeled
// BodyTooLargeError once the cap is exceeded. Keeps draining (discarding) past
// the cap so the 413 response is still deliverable — destroying the socket
// would just reset the client connection with no response at all.
function readBody(req, maxBytes) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    let tooLarge = false;
    req.on("data", (c) => {
      size += c.length;
      if (size > maxBytes) {
        tooLarge = true;
        return; // discard, keep draining
      }
      chunks.push(c);
    });
    req.on("end", () => {
      if (tooLarge) reject(new BodyTooLargeError(maxBytes));
      else resolve(Buffer.concat(chunks).toString("utf8"));
    });
    req.on("error", reject);
  });
}

// clientIp(req) — the real client IP. The gateway sits behind Railway's load
// balancer, so the TCP peer (remoteAddress) is the LB itself and every client
// would share one rate-limit bucket. Railway's LB sets X-Forwarded-For from the
// actual peer for public traffic, so the first entry is the client IP; direct
// private connections (no XFF) fall back to the socket address.
function clientIp(req) {
  const xff = req.headers["x-forwarded-for"];
  if (typeof xff === "string" && xff.trim()) {
    return xff.split(",")[0].trim();
  }
  return (req.socket.remoteAddress || "unknown").replace(/^::ffff:/, "");
}

// startServer({ store, token, port, host, maxBodyBytes, rateLimit }) —
// injectable store makes the whole HTTP path testable against an in-memory
// double. rateLimit = { windowMs, max } (defaults 10s / 5) for the failed-auth
// limiter. Returns the listening server.
export function startServer({
  store,
  token,
  port = 8080,
  host = "0.0.0.0",
  maxBodyBytes = MAX_BODY_BYTES,
  rateLimit,
} = {}) {
  const handle = createHandler({ store, token });
  const limiter = createRateLimiter(rateLimit ?? {});
  const server = http.createServer(async (req, res) => {
    try {
      const ip = clientIp(req);
      if (limiter.isBlocked(ip)) {
        res.writeHead(429, {
          "content-type": "application/json",
          "retry-after": String(Math.ceil(limiter.remainingMs(ip) / 1000)),
        });
        res.end(JSON.stringify({ error: "too many failed attempts" }));
        return;
      }
      const rawBody = await readBody(req, maxBodyBytes);
      let pathname;
      try {
        pathname = new URL(req.url ?? "/", "http://gateway.internal").pathname;
      } catch {
        pathname = req.url ?? "/";
      }
      const out = await handle(req.method, pathname, req.headers, rawBody);
      if (out.status === 401) limiter.recordFailure(ip);
      else limiter.clear(ip);
      res.writeHead(out.status, out.headers);
      res.end(out.body);
    } catch (e) {
      if (e && e.code === "BODY_TOO_LARGE") {
        res.writeHead(413, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: "body too large" }));
        return;
      }
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
  if (token.length < 32) {
    console.error("gateway: GATEWAY_TOKEN must be at least 32 characters");
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
