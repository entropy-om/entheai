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

function respond(status, obj, extraHeaders = {}) {
  return {
    status,
    headers: { "content-type": "application/json", ...extraHeaders },
    body: JSON.stringify(obj),
  };
}
