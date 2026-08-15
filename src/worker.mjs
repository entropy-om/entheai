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
// POST /api/stripe/checkout → hosted Checkout Session at Stripe for the
//                      store.vaked.dev catalog (raw REST, no stripe-node).
// POST /api/sovereign/verify → { key } → Ed25519-verified sovereign token
//                      (offline-verifiable by design; no server lookup).
// GET  /.well-known/sovereign.pub, /api/sovereign/pub → the raw 32-byte
//                      Ed25519 public key (application/octet-stream).
// Everything else    → the static asset pipeline (public/), unchanged.
//
// Bindings (wrangler.jsonc): ASSETS (assets), LICENSES (KV namespace),
// ENTROPY (KV namespace, commented until provisioned), ENTROPY_TOKEN (secret),
// STRIPE_WEBHOOK_SECRET / STRIPE_SECRET_KEY / SOVEREIGN_SIGNING_KEY (secrets
// via `wrangler secret put`). Sovereign tokens live under the `vkd:` KV prefix
// so they can never collide with the live entheai backer keys.

export const SCHEMA = "entheai.entropy.v1";
export const KV_KEY = "entropy:latest";
export const STALE_AFTER_MS = 15 * 60 * 1000;
const MAX_BODY_BYTES = 32 * 1024;
const KV_TTL_SECS = 3600;

// ---- Sovereign key issuance (constellation monetization, Lane 1) ----------
// Ed25519-signed bearer tokens (`vkd_sk_<payload>.<sig>`) minted on store
// checkout and verified offline by any client holding the published public
// key. The signing seed lives in the `SOVEREIGN_SIGNING_KEY` secret; the
// public key below is hardcoded (verified against the seed at deploy time).
// Sovereign records live under the `vkd:` KV prefix, never `license:`.
export const SOVEREIGN_PUBLIC_KEY_B64URL = "BGNHxgOoZVb0BbF_kjIremfSRM_Bmv6Jtgehs2AX96o";
export const SOVEREIGN_TTL_SECS = {
  presence: 30 * 86400, // 30 days
  backer: 365 * 86400, // 1 year
  member: 5 * 365 * 86400, // 5 years
};
export const TIER_ENTITLEMENTS = {
  member: ["community", "gallery-hd", "quantal", "labs", "quant-api", "beta"],
  backer: ["community", "gallery-hd", "quantal", "beta"],
  presence: ["community"],
};
// store.vaked.dev catalog: SKU → price in euro minor units (cents).
export const STORE_CATALOG = {
  tshirt: 3500,
  hoodie: 7500,
  vinyl: 3800,
  print: 12000,
  "audio-bundle": 1500,
  stems: 2500,
  "jazz-vinyl": 4200,
  "sle-print": 8500,
  "mra-hoodie": 7800,
  "nfc-card": 2900,
  "vakedide-supporter": 4900,
  scarf: 6500,
};

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
    if (url.pathname === "/api/stripe/checkout") {
      return handleStripeCheckout(request, env);
    }
    if (url.pathname === "/api/sovereign/verify") {
      return handleSovereignVerify(request, env);
    }
    if (url.pathname === "/.well-known/sovereign.pub" || url.pathname === "/api/sovereign/pub") {
      return handleSovereignPub(request, env);
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

  // Sovereign mint (Lane 1): the constellation store marks its checkouts with
  // metadata[tier]. When present — and not an entheai checkout — ALSO issue a
  // vkd_ token for the same buyer under `vkd:license:<sub>`. Best-effort and
  // additive: a missing SOVEREIGN_SIGNING_KEY must never fail the webhook,
  // and the `session:<id>` short-circuit above keeps this to exactly one mint
  // per session (replays return before reaching here).
  const tier = session.metadata?.tier;
  if (tier && tier !== "entheai-backer" && env.SOVEREIGN_SIGNING_KEY) {
    const sub = session.customer_details?.email || session.metadata?.sub || session.id;
    try {
      const token = await signSovereign(
        buildSovereignPayload(sub, tier),
        env.SOVEREIGN_SIGNING_KEY
      );
      await env.LICENSES.put(`vkd:license:${sub}`, token);
    } catch (e) {
      console.warn(`sovereign mint failed for ${sub}: ${e}`);
    }
  }

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

// ---- Sovereign tokens (Ed25519, WebCrypto-only) ---------------------------

// Base64url (RFC 4648 §5, no padding) — the wire format of vkd_ tokens, the
// public key, and the signing seed.
export function b64url(bytes) {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function b64urlDecode(str) {
  const b64 = str.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (str.length % 4)) % 4);
  const bin = atob(b64);
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}

// Import the 32-byte Ed25519 signing seed. The spec'd raw import
// (`importKey("raw", seed, ..., ["sign"])`) is attempted first; runtimes that
// follow the W3C WebCrypto spec (Node included) treat raw Ed25519 bytes as a
// *public* key and reject the "sign" usage, so we fall back to the RFC 8410
// PKCS#8 wrapper — identical key material, portable to Node and Cloudflare
// workerd alike. The DER is SEQUENCE{INTEGER 0, SEQUENCE{OID 1.3.101.112},
// OCTET STRING{OCTET STRING{seed}}}.
async function importSovereignSigningKey(seedBytes) {
  try {
    return await crypto.subtle.importKey("raw", seedBytes, { name: "Ed25519" }, false, ["sign"]);
  } catch {
    const der = new Uint8Array([
      0x30, 0x2e, 0x02, 0x01, 0x00, // SEQUENCE { INTEGER 0
      0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, // SEQUENCE { OID 1.3.101.112 }
      0x04, 0x22, 0x04, 0x20, // OCTET STRING { OCTET STRING {
      ...seedBytes, // 32-byte seed
    ]);
    return await crypto.subtle.importKey("pkcs8", der, { name: "Ed25519" }, false, ["sign"]);
  }
}

// Build the canonical payload for a sovereign token. Key order is stable so
// the JSON.stringify serialization is deterministic (signed byte-for-byte).
export function buildSovereignPayload(sub, tier, now = Date.now()) {
  const t = Math.floor(now / 1000);
  return {
    v: 1,
    iss: "vaked-sovereign-worker",
    sub,
    tier,
    ent: TIER_ENTITLEMENTS[tier] || TIER_ENTITLEMENTS.presence,
    iat: t,
    exp: t + (SOVEREIGN_TTL_SECS[tier] ?? SOVEREIGN_TTL_SECS.presence),
  };
}

// Sign a payload object with the seed in `seedB64url`, returning
// `vkd_sk_<base64url(payloadJSON)>.<base64url(sig)>`.
export async function signSovereign(payload, seedB64url) {
  const seed = b64urlDecode(seedB64url);
  const key = await importSovereignSigningKey(seed);
  const payloadBytes = new TextEncoder().encode(JSON.stringify(payload));
  const sig = new Uint8Array(await crypto.subtle.sign("Ed25519", key, payloadBytes));
  return `vkd_sk_${b64url(payloadBytes)}.${b64url(sig)}`;
}

// Verify a sovereign token against `pubKeyB64url`: parse, Ed25519-verify the
// signature over the exact signed payload bytes, then check `exp`. Returns the
// decoded payload or null. Signature + exp is the source of truth — no server
// lookup happens here.
async function verifySovereign(token, pubKeyB64url) {
  const match = /^vkd_sk_([A-Za-z0-9_-]+)\.([A-Za-z0-9_-]+)$/.exec(token);
  if (!match) return null;
  let payloadBytes;
  let sigBytes;
  let pubBytes;
  try {
    payloadBytes = b64urlDecode(match[1]);
    sigBytes = b64urlDecode(match[2]);
    pubBytes = b64urlDecode(pubKeyB64url);
  } catch {
    return null;
  }
  try {
    const pub = await crypto.subtle.importKey("raw", pubBytes, { name: "Ed25519" }, false, [
      "verify",
    ]);
    const valid = await crypto.subtle.verify("Ed25519", pub, sigBytes, payloadBytes);
    if (!valid) return null;
  } catch {
    return null;
  }
  let payload;
  try {
    payload = JSON.parse(new TextDecoder().decode(payloadBytes));
  } catch {
    return null;
  }
  if (!payload || typeof payload.sub !== "string" || typeof payload.exp !== "number") {
    return null;
  }
  if (payload.exp <= Math.floor(Date.now() / 1000)) return null;
  return payload;
}

// GET /.well-known/sovereign.pub (and /api/sovereign/pub) → the raw 32-byte
// Ed25519 public key. Public, cacheable, deliberately not JSON.
export async function handleSovereignPub(request, env) {
  const pubB64 = env.SOVEREIGN_PUBLIC_KEY || SOVEREIGN_PUBLIC_KEY_B64URL;
  let bytes;
  try {
    bytes = b64urlDecode(pubB64);
  } catch {
    return json({ error: "public key unconfigured" }, 503, jsonHeaders());
  }
  return new Response(bytes, {
    status: 200,
    headers: {
      "content-type": "application/octet-stream",
      "access-control-allow-origin": "*",
      "cache-control": "public, max-age=3600",
    },
  });
}

// POST /api/stripe/checkout — create a hosted Checkout Session for the
// store.vaked.dev catalog via raw Stripe REST (no stripe-node). Returns
// `{ url }` or the Stripe error (status + message).
export async function handleStripeCheckout(request, env) {
  const headers = jsonHeaders();
  if (!env.STRIPE_SECRET_KEY) {
    return json({ error: "stripe not configured" }, 503, headers);
  }
  if (request.method !== "POST") {
    return json({ error: "method not allowed" }, 405, { ...headers, allow: "POST" });
  }
  let body;
  try {
    body = JSON.parse(await request.text());
  } catch {
    return json({ error: "body must be JSON" }, 400, headers);
  }
  if (!Array.isArray(body.items) || body.items.length === 0) {
    return json({ error: "items required" }, 400, headers);
  }
  let tier = typeof body.tier === "string" && body.tier ? body.tier : "backer";
  const form = new URLSearchParams();
  form.set("mode", "payment");
  form.set("success_url", "https://store.vaked.dev/?checkout={CHECKOUT_SESSION_ID}");
  form.set("cancel_url", "https://store.vaked.dev/?canceled=1");
  form.set("metadata[source]", "store.vaked.dev");
  // This account has Managed Payments on by default, which requires a product
  // tax_code for inline price_data. We're selling simple digital/merch SKUs, so
  // disable Managed Payments on the session rather than classify each SKU.
  form.set("managed_payments[enabled]", "false");
  for (let i = 0; i < body.items.length; i++) {
    const item = body.items[i];
    const sku = typeof item.sku === "string" ? item.sku.trim() : "";
    if (!sku) return json({ error: "item sku required" }, 400, headers);
    const skuLower = sku.toLowerCase();
    if (skuLower.includes("vakedide") || skuLower.includes("supporter")) {
      tier = "member";
    }
    let price;
    if (item.unit_amount !== undefined) {
      if (!Number.isInteger(item.unit_amount) || item.unit_amount <= 0) {
        return json({ error: `invalid unit_amount for ${sku}` }, 400, headers);
      }
      price = item.unit_amount;
    } else {
      price = STORE_CATALOG[skuLower];
      if (typeof price !== "number") {
        return json({ error: `unknown sku: ${sku}` }, 400, headers);
      }
    }
    const qty = Number.isInteger(item.qty) && item.qty > 0 ? item.qty : 1;
    const p = `line_items[${i}]`;
    form.set(`${p}[quantity]`, String(qty));
    form.set(`${p}[price_data][currency]`, "eur");
    form.set(`${p}[price_data][product_data][name]`, sku);
    form.set(`${p}[price_data][unit_amount]`, String(price));
  }
  form.set("metadata[tier]", tier);
  let res;
  try {
    res = await fetch("https://api.stripe.com/v1/checkout/sessions", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${env.STRIPE_SECRET_KEY}`,
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body: form.toString(),
    });
  } catch {
    return json({ error: "stripe unreachable" }, 502, headers);
  }
  let data;
  try {
    data = await res.json();
  } catch {
    data = {};
  }
  if (!res.ok) {
    return json(
      { error: data?.error?.message || `stripe error ${res.status}` },
      res.status || 502,
      headers
    );
  }
  if (typeof data.url !== "string" || !data.url) {
    return json({ error: "stripe returned no url" }, 502, headers);
  }
  return json({ url: data.url }, 200, headers);
}

// POST /api/sovereign/verify — { key } → offline Ed25519 verification against
// the published public key. No auth (the key IS the credential). When the KV
// store is bound, the token's `vkd:license:<sub>` record is cross-checked as a
// revocation signal (missing → rejected); signature + exp remain the source of
// truth when no store is bound.
export async function handleSovereignVerify(request, env) {
  const headers = jsonHeaders();
  if (request.method !== "POST") {
    return json({ error: "method not allowed" }, 405, { ...headers, allow: "POST" });
  }
  let key = "";
  try {
    const body = JSON.parse(await request.text());
    key = typeof body.key === "string" ? body.key.trim() : "";
  } catch {
    return json({ ok: false }, 401, headers);
  }
  if (!key) return json({ ok: false }, 401, headers);
  const payload = await verifySovereign(key, env.SOVEREIGN_PUBLIC_KEY || SOVEREIGN_PUBLIC_KEY_B64URL);
  if (!payload) return json({ ok: false }, 401, headers);
  if (env.LICENSES) {
    const raw = await env.LICENSES.get(`vkd:license:${payload.sub}`);
    if (!raw) return json({ ok: false }, 401, headers);
  }
  return json(
    { ok: true, tier: payload.tier, ent: payload.ent, sub: payload.sub, exp: payload.exp },
    200,
    headers
  );
}

function json(obj, status, headers) {
  return new Response(JSON.stringify(obj), { status, headers });
}
