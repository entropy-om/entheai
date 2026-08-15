// entheai.com Worker — static assets + the live entropy beacon (roadmap 4.1)
// + Stripe-backed backer licenses.
//
// GET  /api/entropy  → the latest EntropySnapshot from KV, wrapped as
//                      { live, stale?, snapshot? }. `live` is false when no
//                      snapshot exists or the newest one is older than
//                      STALE_AFTER_MS — the site never fakes liveness.
// POST /api/entropy  → authenticated write path for the local bridge:
//                      `Authorization: Bearer <ENTROPY_TOKEN>` + a JSON body
//                      whose `schema` is exactly "entheai.entropy.v1".
// POST /api/stripe/webhook → verify Stripe-Signature (HMAC-SHA256 via
//                      crypto.subtle, no stripe-node) and fulfill
//                      checkout.session.completed into the LICENSES KV
//                      (idempotent on session:<checkout id>).
// POST /api/license/verify → { key } → entitlement check. No auth (the key IS
//                      the credential).
// GET  /api/license/claim?session_id= → key for a fulfilled session. No auth.
// GET  /api/releases?channel=beta → beta manifest seam (releases:<channel>,
//                      value written later by release tooling).
// Everything else    → the static asset pipeline (public/), unchanged.
//
// Bindings (wrangler.jsonc): ASSETS (assets), LICENSES (KV namespace),
// ENTROPY (KV namespace, commented until provisioned), ENTROPY_TOKEN (secret),
// STRIPE_WEBHOOK_SECRET (secret via `wrangler secret put STRIPE_WEBHOOK_SECRET`).

export const SCHEMA = "entheai.entropy.v1";
export const KV_KEY = "entropy:latest";
export const STALE_AFTER_MS = 15 * 60 * 1000;
const MAX_BODY_BYTES = 32 * 1024;
const KV_TTL_SECS = 3600;

// Backer license keys: ENTH-XXXX-XXXX-XXXX-XXXX — 16 chars drawn from an
// unambiguous alphabet (no 0/O/1/I/L), generated with crypto.getRandomValues.
export const LICENSE_ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const LICENSE_GROUPS = 4;
const LICENSE_GROUP_LEN = 4;
const LICENSE_BODY_LEN = LICENSE_GROUPS * LICENSE_GROUP_LEN;

export function generateLicenseKey() {
  const alphabetLen = LICENSE_ALPHABET.length;
  const maxUnbiased = Math.floor(256 / alphabetLen) * alphabetLen;
  let key = "ENTH-";
  let produced = 0;

  while (produced < LICENSE_BODY_LEN) {
    const byte = crypto.getRandomValues(new Uint8Array(1))[0];
    if (byte >= maxUnbiased) continue;
    if (produced > 0 && produced % LICENSE_GROUP_LEN === 0) key += "-";
    key += LICENSE_ALPHABET[byte % alphabetLen];
    produced++;
  }

  return key;
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    // gallery.entheai.com/ → serve the gallery at the root. Rewrite to the
    // PRETTY path "/gallery" (which the assets pipeline serves as gallery.html,
    // 200) — NOT "/gallery.html", which html_handling 307-redirects back to
    // /gallery and loops. Paired with assets.run_worker_first=["/"] so the
    // Worker runs for the root at all (assets are served first by default).
    // /gallery itself is left to the assets pretty-URL handler (200, no Worker).
    if (url.hostname === "gallery.entheai.com" && url.pathname === "/") {
      url.pathname = "/gallery";
      return env.ASSETS.fetch(new Request(url, request));
    }
    if (url.pathname === "/api/entropy") {
      return handleEntropy(request, env);
    }
    if (url.pathname === "/api/stripe/webhook") {
      return handleStripe(request, env);
    }
    if (url.pathname === "/api/license/verify") {
      return handleLicense(request, env);
    }
    if (url.pathname === "/api/license/claim") {
      return handleClaim(request, env);
    }
    if (url.pathname === "/api/releases") {
      return handleReleases(request, env);
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

// ---- Stripe monetization (backer licenses) --------------------------------

// Constant-time compare of two lowercase hex strings (timing-safe).
function timingSafeEqualHex(a, b) {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

// Manual Stripe webhook signature check (no stripe-node). The signed payload
// is `t + "." + rawBody` — the raw UTF-8 request text, never re-serialized.
async function verifyStripeSignature(rawBody, signatureHeader, secret) {
  if (!signatureHeader || !secret) return false;
  const parts = {};
  for (const piece of signatureHeader.split(",")) {
    const eq = piece.indexOf("=");
    if (eq === -1) continue;
    parts[piece.slice(0, eq).trim()] = piece.slice(eq + 1).trim();
  }
  const t = parts.t;
  const v1 = parts.v1;
  if (!t || !v1) return false;
  const ts = Number(t);
  // Reject timestamps more than 300s away from now (a future timestamp is
  // equally outside the replay window).
  if (!Number.isFinite(ts) || Math.abs(Date.now() / 1000 - ts) > 300) return false;
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  const mac = await crypto.subtle.sign(
    "HMAC",
    key,
    new TextEncoder().encode(`${t}.${rawBody}`)
  );
  const expected = [...new Uint8Array(mac)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return timingSafeEqualHex(expected, v1.toLowerCase());
}

function jsonHeaders(extra = {}) {
  return {
    "content-type": "application/json",
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
    ...extra,
  };
}

// POST /api/stripe/webhook — verify the signature, then fulfill
// checkout.session.completed into LICENSES. Idempotent per checkout session.
export async function handleStripe(request, env) {
  const headers = jsonHeaders();
  if (!env.STRIPE_WEBHOOK_SECRET) {
    return json({ error: "webhook secret unconfigured" }, 503, headers);
  }
  if (!env.LICENSES) {
    return json({ error: "licenses store unbound" }, 503, headers);
  }
  if (request.method !== "POST") {
    return json({ error: "method not allowed" }, 405, { ...headers, allow: "POST" });
  }
  const rawBody = await request.text();
  let valid = false;
  try {
    valid = await verifyStripeSignature(
      rawBody,
      request.headers.get("stripe-signature"),
      env.STRIPE_WEBHOOK_SECRET
    );
  } catch {
    valid = false;
  }
  if (!valid) {
    return json({ error: "invalid signature" }, 400, headers);
  }
  let event;
  try {
    event = JSON.parse(rawBody);
  } catch {
    return json({ error: "invalid signature" }, 400, headers);
  }
  if (event.type !== "checkout.session.completed") {
    return json({ received: true }, 200, headers);
  }
  const session = event.data?.object;
  if (!session || !session.id || session.payment_status !== "paid") {
    return json({ received: true }, 200, headers);
  }
  // Idempotency: fulfill a checkout session exactly once.
  const existing = await env.LICENSES.get(`session:${session.id}`);
  if (existing) {
    return json({ received: true }, 200, headers);
  }
  const key = generateLicenseKey();
  const license = JSON.stringify({
    session_id: session.id,
    email: session.customer_details?.email ?? "",
    product: "entheai-backer",
    created_at: Date.now(),
    entitlements: ["beta"],
  });
  await env.LICENSES.put(`license:${key}`, license);
  await env.LICENSES.put(`session:${session.id}`, key);

  // Best-effort email delivery: the license is already durable in KV, so a
  // failed send must never fail the webhook (Stripe would retry and we'd hit
  // the idempotency short-circuit above). Skip silently when unconfigured.
  const email = session.customer_details?.email;
  if (email) {
    await sendBackerEmail(env, email, key);
  }
  return json({ received: true }, 200, headers);
}

// Send the freshly-minted license key to the buyer via AgentMail. Best-effort:
// logs a warning on any failure and never throws. Requires the AgentMail API
// key (`AGENTMAIL_AGENTMAIL_API_KEY`) and a created inbox (`AGENTMAIL_INBOX_ID`).
export async function sendBackerEmail(env, to, key) {
  if (!env.AGENTMAIL_AGENTMAIL_API_KEY || !env.AGENTMAIL_INBOX_ID) {
    return;
  }
  const body = JSON.stringify({
    to,
    subject: "Your entheai Backer license key",
    text: [
      "Thanks for backing entheai.",
      "",
      `Your license key: ${key}`,
      "",
      "Activate it with:",
      `  entheai activate ${key}`,
      "",
      "The beta channel unlocks once activated. If you lose this key, you can",
      "reclaim it from https://entheai.com/back/claim",
    ].join("\n"),
  });
  try {
    await fetch(
      `https://api.agentmail.to/v0/inboxes/${env.AGENTMAIL_INBOX_ID}/messages/send`,
      {
        method: "POST",
        headers: {
          Authorization: `Bearer ${env.AGENTMAIL_AGENTMAIL_API_KEY}`,
          "Content-Type": "application/json",
        },
        body,
      }
    );
  } catch (e) {
    // Delivery is best-effort; the claim page is the fallback path.
    console.warn(`sendBackerEmail failed: ${e}`);
  }
}

// POST /api/license/verify — { key } → entitlement. No auth (the key IS the
// credential).
export async function handleLicense(request, env) {
  const headers = jsonHeaders();
  if (!env.LICENSES) {
    return json({ error: "licenses store unbound" }, 503, headers);
  }
  if (request.method !== "POST") {
    return json({ error: "method not allowed" }, 405, { ...headers, allow: "POST" });
  }
  let key = "";
  try {
    const body = JSON.parse(await request.text());
    key = typeof body.key === "string" ? body.key.trim().toUpperCase() : "";
  } catch {
    return json({ ok: false }, 401, headers);
  }
  if (!key) return json({ ok: false }, 401, headers);
  const raw = await env.LICENSES.get(`license:${key}`);
  if (!raw) return json({ ok: false }, 401, headers);
  let license;
  try {
    license = JSON.parse(raw);
  } catch {
    return json({ ok: false }, 401, headers);
  }
  return json(
    {
      ok: true,
      backer: true,
      entitlements: Array.isArray(license.entitlements) ? license.entitlements : ["beta"],
      email: typeof license.email === "string" ? license.email : "",
    },
    200,
    headers
  );
}

// GET /api/license/claim?session_id=cs_... → key for a fulfilled session.
export async function handleClaim(request, env) {
  const headers = jsonHeaders();
  if (!env.LICENSES) {
    return json({ error: "licenses store unbound" }, 503, headers);
  }
  if (request.method !== "GET") {
    return json({ error: "method not allowed" }, 405, { ...headers, allow: "GET" });
  }
  const sessionId = new URL(request.url).searchParams.get("session_id");
  if (!sessionId) return json({ error: "not found or not yet fulfilled" }, 404, headers);
  const key = await env.LICENSES.get(`session:${sessionId}`);
  if (!key) return json({ error: "not found or not yet fulfilled" }, 404, headers);
  return json({ key }, 200, headers);
}

// GET /api/releases?channel=beta — beta manifest seam. The value for
// `releases:<channel>` is written later by release tooling; here we only read.
export async function handleReleases(request, env) {
  const headers = jsonHeaders();
  if (!env.LICENSES) {
    return json({ error: "licenses store unbound" }, 503, headers);
  }
  if (request.method !== "GET") {
    return json({ error: "method not allowed" }, 405, { ...headers, allow: "GET" });
  }
  const channel = new URL(request.url).searchParams.get("channel") || "beta";
  const raw = await env.LICENSES.get(`releases:${channel}`);
  if (!raw) return json({ version: null }, 200, headers);
  try {
    return json(JSON.parse(raw), 200, headers);
  } catch {
    return json({ version: null }, 200, headers);
  }
}

function json(obj, status, headers) {
  return new Response(JSON.stringify(obj), { status, headers });
}
