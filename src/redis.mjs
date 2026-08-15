// src/redis.mjs — minimal RESP2 client for Cloudflare Workers.
//
// Replaces the retired Cloudflare KV LICENSES namespace: the backer license
// store now lives in Railway Redis, reached over a raw TCP socket via the
// Workers `connect()` API (GA, no compat flags). No npm deps.
//
// The `connect` import is deliberately dynamic (inside redis()) so the pure
// encode/decode helpers are importable + testable in plain Node.

const enc = new TextEncoder();
const dec = new TextDecoder();

// encodeCommand(...parts) -> RESP2 command bytes.
//   GET key            => *2\r\n$3\r\nGET\r\n$3\r\nkey\r\n
//   SET key value      => *3\r\n$3\r\nSET\r\n$3\r\nkey\r\n$5\r\nvalue\r\n
//   AUTH user pass     => *3\r\n$4\r\nAUTH\r\n$4\r\nuser\r\n$8\r\npass\r\n
//   SETEX key 60 val   => *4\r\n$5\r\nSETEX\r\n$3\r\nkey\r\n$2\r\n60\r\n$5\r\nval\r\n
export function encodeCommand(...parts) {
  let out = `*${parts.length}\r\n`;
  for (const p of parts) {
    const s = String(p);
    out += `$${s.length}\r\n${s}\r\n`;
  }
  return enc.encode(out);
}

function findCRLF(b) {
  for (let i = 0; i < b.length - 1; i++) {
    if (b[i] === 13 && b[i + 1] === 10) return i;
  }
  return -1;
}

function concat(a, b) {
  const o = new Uint8Array(a.length + b.length);
  o.set(a);
  o.set(b, a.length);
  return o;
}

// parseReply(reader) -> string | number | null. Handles the RESP2 reply types
// the license store needs: +simple, -error (throws), :integer, $bulk ($-1=null).
export async function parseReply(reader) {
  let buf = new Uint8Array(0);
  const more = async () => {
    const { value, done } = await reader.read();
    if (done || !value) throw new Error("redis: socket closed mid-reply");
    buf = concat(buf, value);
  };
  const line = async () => {
    for (;;) {
      const i = findCRLF(buf);
      if (i >= 0) {
        const s = dec.decode(buf.slice(0, i));
        buf = buf.slice(i + 2);
        return s;
      }
      await more();
    }
  };

  const l = await line();
  const t = l[0];
  const rest = l.slice(1);
  if (t === "+") return rest; // e.g. +OK
  if (t === "-") throw new Error("redis: " + rest);
  if (t === ":") return Number(rest);
  if (t === "$") {
    const n = Number(rest);
    if (n === -1) return null; // nil bulk string (missing key)
    while (buf.length < n + 2) await more();
    const v = dec.decode(buf.slice(0, n));
    buf = buf.slice(n + 2);
    return v;
  }
  throw new Error("redis: unhandled RESP type " + t);
}

// redis(url, ...parts) -> string | number | null. One socket per request:
// connect -> (AUTH) -> command -> read reply -> close. `url` is
// new URL(env.REDIS_PUBLIC_URL); use rediss:// to enable TLS.
export async function redis(url, ...parts) {
  if (!url) throw new Error("redis: no REDIS_PUBLIC_URL");
  const { connect } = await import("cloudflare:sockets");
  const socket = connect(
    { hostname: url.hostname, port: Number(url.port) || 6379 },
    { secureTransport: url.protocol === "rediss:" ? "on" : "off" }
  );
  try {
    await socket.opened;
    const w = socket.writable.getWriter();
    const r = socket.readable.getReader();
    const pass = url.password ? decodeURIComponent(url.password) : undefined;
    if (pass) {
      await w.write(
        encodeCommand("AUTH", url.username ? decodeURIComponent(url.username) : "default", pass)
      );
      await parseReply(r); // +OK (throws on wrong password)
    }
    await w.write(encodeCommand(...parts));
    const res = await parseReply(r);
    await w.close();
    return res;
  } finally {
    await socket.close();
  }
}
