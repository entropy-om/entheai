// entheai.com Worker — static assets + the live entropy beacon (roadmap 4.1).
//
// GET  /api/entropy  → the latest EntropySnapshot from KV, wrapped as
//                      { live, stale?, snapshot? }. `live` is false when no
//                      snapshot exists or the newest one is older than
//                      STALE_AFTER_MS — the site never fakes liveness.
// POST /api/entropy  → authenticated write path for the local bridge:
//                      `Authorization: Bearer <ENTROPY_TOKEN>` + a JSON body
//                      whose `schema` is exactly "entheai.entropy.v1".
// Everything else    → the static asset pipeline (public/), unchanged.
//
// Bindings (wrangler.jsonc): ASSETS (assets), ENTROPY (KV namespace),
// ENTROPY_TOKEN (secret via `wrangler secret put ENTROPY_TOKEN`).

export const SCHEMA = "entheai.entropy.v1";
export const KV_KEY = "entropy:latest";
export const STALE_AFTER_MS = 15 * 60 * 1000;
const MAX_BODY_BYTES = 32 * 1024;
const KV_TTL_SECS = 3600;

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/api/entropy") {
      return handleEntropy(request, env);
    }
    return env.ASSETS.fetch(request);
  },
};

export async function handleEntropy(request, env, now = Date.now) {
  const headers = {
    "content-type": "application/json",
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
  };
  if (!env.ENTROPY) {
    return json({ error: "entropy store unbound" }, 503, headers);
  }
  if (request.method === "GET") {
    const raw = await env.ENTROPY.get(KV_KEY);
    if (!raw) {
      return json({ live: false }, 200, headers);
    }
    let snapshot;
    try {
      snapshot = JSON.parse(raw);
    } catch {
      return json({ live: false }, 200, headers);
    }
    const stale =
      typeof snapshot.at_ms !== "number" || now() - snapshot.at_ms > STALE_AFTER_MS;
    return json({ live: !stale, stale, snapshot }, 200, headers);
  }
  if (request.method === "POST") {
    const auth = request.headers.get("authorization") || "";
    if (!env.ENTROPY_TOKEN || auth !== `Bearer ${env.ENTROPY_TOKEN}`) {
      return json({ error: "unauthorized" }, 401, headers);
    }
    const body = await request.text();
    if (body.length > MAX_BODY_BYTES) {
      return json({ error: "body too large" }, 413, headers);
    }
    let snapshot;
    try {
      snapshot = JSON.parse(body);
    } catch {
      return json({ error: "body must be JSON" }, 400, headers);
    }
    if (snapshot.schema !== SCHEMA) {
      return json({ error: `schema must be ${SCHEMA}` }, 422, headers);
    }
    await env.ENTROPY.put(KV_KEY, JSON.stringify(snapshot), {
      expirationTtl: KV_TTL_SECS,
    });
    return json({ ok: true }, 200, headers);
  }
  return json({ error: "method not allowed" }, 405, { ...headers, allow: "GET, POST" });
}

function json(obj, status, headers) {
  return new Response(JSON.stringify(obj), { status, headers });
}
