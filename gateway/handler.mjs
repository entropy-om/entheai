// gateway/handler.mjs — transport-agnostic HTTP handler for the Redis gateway.
//
// Deliberately free of node:http / node:net so the auth + routing logic is
// unit-testable against a fake in-memory store (see test/gateway.test.mjs).
// `store` only needs the Redis-ish surface the routes touch:
//   { get(key), set(key, value), del(key), ping() }
//
// Auth: every request must carry `Authorization: Bearer <GATEWAY_TOKEN>`, else
// 401. Comparison is constant-time (crypto.timingSafeEqual).
//
// Routes (all under /):
//   GET    /:key     → 200 {"value":"..."} | 404 {"error":"not found"}
//   PUT    /:key     → body is the raw string value → 200 {"ok":true}
//   DELETE /:key     → 200 {"ok":true}
//   GET    /health   → pings Redis → 200 {"ok":true} | 503 {"error":"redis unreachable"}
//
// Returns { status, headers, body } where body is a JSON string.

import { timingSafeEqual } from "node:crypto";

export function createHandler({ store, token }) {
  if (!store) throw new Error("handler: a store is required");
  if (!token) throw new Error("handler: a GATEWAY_TOKEN is required");

  return async function handle(method, pathname, headers = {}, rawBody = "") {
    if (!authorized(headers?.authorization ?? headers?.Authorization ?? "", token)) {
      return respond(401, { error: "unauthorized" });
    }

    if (pathname === "/health") {
      if (method !== "GET") {
        return respond(405, { error: "method not allowed" }, { allow: "GET" });
      }
      try {
        await store.ping();
        return respond(200, { ok: true });
      } catch {
        return respond(503, { error: "redis unreachable" });
      }
    }

    // /:key — everything else under / is a key route. Keys are sent
    // percent-encoded (the Worker encodes with encodeURIComponent); decode here.
    let key;
    try {
      key = decodeURIComponent(pathname.startsWith("/") ? pathname.slice(1) : pathname);
    } catch {
      return respond(400, { error: "invalid key encoding" });
    }

    if (method === "GET") {
      const value = await store.get(key);
      if (value === null || value === undefined) {
        return respond(404, { error: "not found" });
      }
      return respond(200, { value: String(value) });
    }
    if (method === "PUT") {
      await store.set(key, String(rawBody ?? ""));
      return respond(200, { ok: true });
    }
    if (method === "DELETE") {
      await store.del(key);
      return respond(200, { ok: true });
    }
    return respond(405, { error: "method not allowed" }, { allow: "GET, PUT, DELETE" });
  };
}

function authorized(header, token) {
  if (!header || !token) return false;
  const match = /^Bearer (.+)$/i.exec(header.trim());
  if (!match) return false;
  const provided = Buffer.from(match[1]);
  const expected = Buffer.from(token);
  // timingSafeEqual throws on length mismatch, so length-check first.
  return provided.length === expected.length && timingSafeEqual(provided, expected);
}

// createRateLimiter({ windowMs, max }) — a tiny in-memory failed-auth limiter,
// keyed by client IP. `max` failed attempts per `windowMs` are allowed; the
// next attempt is blocked (429) until the window rolls over. A successful
// request clears the budget via clear(). Pure data structure — the HTTP layer
// decides when to call it (see server.mjs).
export function createRateLimiter({ windowMs = 10_000, max = 5 } = {}) {
  const buckets = new Map(); // ip -> { count, resetAt }

  return {
    // True once the ip has exhausted its failed-auth budget in the window.
    isBlocked(ip) {
      const b = buckets.get(ip);
      return !!b && Date.now() < b.resetAt && b.count >= max;
    },
    // Record one failed auth attempt for ip (fresh window on first failure).
    recordFailure(ip) {
      const now = Date.now();
      const b = buckets.get(ip);
      if (!b || now >= b.resetAt) {
        buckets.set(ip, { count: 1, resetAt: now + windowMs });
      } else {
        b.count += 1;
      }
    },
    // A successful auth clears the budget for ip.
    clear(ip) {
      buckets.delete(ip);
    },
    // Milliseconds until the current window rolls over (for Retry-After).
    remainingMs(ip) {
      const b = buckets.get(ip);
      if (!b) return 0;
      return Math.max(0, b.resetAt - Date.now());
    },
  };
}

function respond(status, obj, extraHeaders = {}) {
  return {
    status,
    headers: { "content-type": "application/json", ...extraHeaders },
    body: JSON.stringify(obj),
  };
}
