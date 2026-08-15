import test from "node:test";
import assert from "node:assert/strict";
import worker, {
  handleEntropy,
  handleStripe,
  handleLicense,
  handleClaim,
  handleReleases,
  handleStripeCheckout,
  handleSovereignVerify,
  handleSovereignPub,
  generateLicenseKey,
  sendBackerEmail,
  signSovereign,
  buildSovereignPayload,
  b64url,
  b64urlDecode,
  LICENSE_ALPHABET,
  SCHEMA,
  KV_KEY,
  STALE_AFTER_MS,
  SOVEREIGN_PUBLIC_KEY_B64URL,
  SOVEREIGN_TTL_SECS,
  TIER_ENTITLEMENTS,
  STORE_CATALOG,
} from "../src/worker.mjs";
import { redisHttp } from "../src/redis.mjs";

/** Minimal in-memory KV double (get/put are all the worker uses). */
function fakeKv() {
  const store = new Map();
  return {
    store,
    async get(k) {
      return store.has(k) ? store.get(k) : null;
    },
    async put(k, v) {
      store.set(k, v);
    },
  };
}

/** Minimal in-memory license-store double (get/set are all the worker uses). */
function fakeStore() {
  const store = new Map();
  return {
    store,
    async get(k) {
      return store.has(k) ? store.get(k) : null;
    },
    async set(k, v) {
      store.set(k, v);
    },
  };
}

function env(overrides = {}) {
  return { ENTROPY: fakeKv(), ENTROPY_TOKEN: "sekrit", ...overrides };
}

function post(body, token = "sekrit") {
  return new Request("https://entheai.com/api/entropy", {
    method: "POST",
    headers: token ? { authorization: `Bearer ${token}` } : {},
    body,
  });
}

const GET = new Request("https://entheai.com/api/entropy");

function snapshot(at_ms = Date.now()) {
  return { schema: SCHEMA, session: "s1", at_ms, glow: [["Model", 0.8]], workers: 2 };
}

test("GET with no snapshot reports live:false — the site never fakes liveness", async () => {
  const res = await handleEntropy(GET, env());
  assert.equal(res.status, 200);
  assert.deepEqual(await res.json(), { live: false });
});

test("POST requires the bearer token", async () => {
  const e = env();
  assert.equal((await handleEntropy(post(JSON.stringify(snapshot()), "wrong"), e)).status, 401);
  assert.equal((await handleEntropy(post(JSON.stringify(snapshot()), null), e)).status, 401);
  // An unset secret rejects everything — no token, no writes.
  const noToken = env({ ENTROPY_TOKEN: undefined });
  assert.equal((await handleEntropy(post(JSON.stringify(snapshot())), noToken)).status, 401);
});

test("POST validates schema and JSON before writing", async () => {
  const e = env();
  assert.equal((await handleEntropy(post("not json"), e)).status, 400);
  const wrong = JSON.stringify({ ...snapshot(), schema: "entheai.entropy.v999" });
  assert.equal((await handleEntropy(post(wrong), e)).status, 422);
  assert.equal(e.ENTROPY.store.size, 0, "nothing written on rejection");
});

test("POST → GET round trip is live; old snapshots go stale honestly", async () => {
  const e = env();
  const fresh = snapshot();
  assert.equal((await handleEntropy(post(JSON.stringify(fresh)), e)).status, 200);
  assert.equal(e.ENTROPY.store.has(KV_KEY), true);

  const live = await (await handleEntropy(GET, e)).json();
  assert.equal(live.live, true);
  assert.equal(live.stale, false);
  assert.deepEqual(live.snapshot, fresh);

  // Same snapshot viewed from beyond the staleness horizon.
  const later = () => fresh.at_ms + STALE_AFTER_MS + 1;
  const stale = await (await handleEntropy(GET, e, later)).json();
  assert.equal(stale.live, false);
  assert.equal(stale.stale, true);
});

test("unbound KV yields 503, other methods 405", async () => {
  assert.equal((await handleEntropy(GET, { ENTROPY_TOKEN: "x" })).status, 503);
  const del = new Request("https://entheai.com/api/entropy", { method: "DELETE" });
  const res = await handleEntropy(del, env());
  assert.equal(res.status, 405);
  assert.equal(res.headers.get("allow"), "GET, POST");
});

test("gallery.entheai.com root routes to the pretty /gallery", async () => {
  let fetchedUrl = null;
  const mockEnv = {
    ASSETS: {
      async fetch(req) {
        fetchedUrl = req.url;
        return new Response("gallery html", { status: 200 });
      },
    },
  };
  const req = new Request("https://gallery.entheai.com/");
  const res = await worker.fetch(req, mockEnv);
  assert.equal(res.status, 200);
  // The PRETTY path, not "/gallery.html": html_handling 307-redirects
  // /gallery.html → /gallery, which would loop. The assets pipeline serves
  // gallery.html for /gallery directly (200).
  assert.equal(fetchedUrl, "https://gallery.entheai.com/gallery");
});

// ---- Stripe monetization (backer licenses) --------------------------------

const WHSEC = "whsec_test_secret";

function licenseEnv(overrides = {}) {
  return { __store: fakeStore(), STRIPE_WEBHOOK_SECRET: WHSEC, ...overrides };
}

/** Sign `${t}.${body}` the way Stripe does, so the worker's own HMAC path runs. */
async function stripeSignature(body, secret = WHSEC, t = Math.floor(Date.now() / 1000)) {
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  const mac = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(`${t}.${body}`));
  const v1 = [...new Uint8Array(mac)].map((b) => b.toString(16).padStart(2, "0")).join("");
  return `t=${t},v1=${v1}`;
}

async function webhook(body, { secret = WHSEC, t, signature } = {}) {
  const sig = signature ?? (await stripeSignature(body, secret, t));
  return new Request("https://entheai.com/api/stripe/webhook", {
    method: "POST",
    headers: sig ? { "stripe-signature": sig } : {},
    body,
  });
}

function checkoutEvent(overrides = {}) {
  return JSON.stringify({
    type: "checkout.session.completed",
    data: {
      object: {
        id: "cs_test_123",
        payment_status: "paid",
        customer_details: { email: "backer@example.com" },
        ...overrides,
      },
    },
  });
}

test("license keys match ENTH-XXXX-XXXX-XXXX-XXXX over the unambiguous alphabet", () => {
  assert.equal(/[01OIL]/.test(LICENSE_ALPHABET), false, "no ambiguous glyphs");
  const keys = new Set();
  for (let i = 0; i < 200; i++) {
    const key = generateLicenseKey();
    assert.match(key, /^ENTH-[A-Z2-9]{4}-[A-Z2-9]{4}-[A-Z2-9]{4}-[A-Z2-9]{4}$/);
    for (const ch of key.slice(5).replace(/-/g, "")) {
      assert.ok(LICENSE_ALPHABET.includes(ch), `${ch} outside the alphabet`);
    }
    keys.add(key);
  }
  assert.equal(keys.size, 200, "keys are unique");
});

test("license keys draw every symbol — rejection sampling stays unbiased", () => {
  // 31 symbols don't divide 256, so a plain modulo would starve nothing but
  // over-weight the first 8; sampling widely must still reach all 31.
  const seen = new Set();
  for (let i = 0; i < 4000; i++) {
    for (const ch of generateLicenseKey().slice(5).replace(/-/g, "")) seen.add(ch);
  }
  assert.equal(seen.size, LICENSE_ALPHABET.length);
});

test("webhook refuses to run unconfigured — no secret, no store, no GET", async () => {
  const body = checkoutEvent();
  assert.equal(
    (await handleStripe(await webhook(body), licenseEnv({ STRIPE_WEBHOOK_SECRET: undefined })))
      .status,
    503
  );
  assert.equal(
    (await handleStripe(await webhook(body), licenseEnv({ __store: undefined }))).status,
    503
  );
  const get = new Request("https://entheai.com/api/stripe/webhook");
  const res = await handleStripe(get, licenseEnv());
  assert.equal(res.status, 405);
  assert.equal(res.headers.get("allow"), "POST");
});

test("webhook rejects unsigned, mis-signed, tampered, and replayed deliveries", async () => {
  const body = checkoutEvent();
  const cases = {
    missing: await webhook(body, { signature: "" }),
    malformed: await webhook(body, { signature: "not-a-signature" }),
    "wrong secret": await webhook(body, { secret: "whsec_other" }),
    // Signature computed over a different payload than the one delivered.
    tampered: new Request("https://entheai.com/api/stripe/webhook", {
      method: "POST",
      headers: { "stripe-signature": await stripeSignature(body) },
      body: checkoutEvent({ id: "cs_evil" }),
    }),
    // Outside the 300s replay window, in both directions.
    stale: await webhook(body, { t: Math.floor(Date.now() / 1000) - 400 }),
    future: await webhook(body, { t: Math.floor(Date.now() / 1000) + 400 }),
  };
  for (const [name, req] of Object.entries(cases)) {
    const e = licenseEnv();
    const res = await handleStripe(req, e);
    assert.equal(res.status, 400, `${name} must be rejected`);
    assert.equal(e.__store.store.size, 0, `${name} must not fulfill`);
  }
});

test("a signed paid checkout mints one license, idempotently", async () => {
  const e = licenseEnv();
  const body = checkoutEvent();
  const first = await handleStripe(await webhook(body), e);
  assert.equal(first.status, 200);
  assert.deepEqual(await first.json(), { received: true });
  assert.equal(first.headers.get("content-type"), "application/json");
  assert.equal(first.headers.get("cache-control"), "no-store");
  assert.equal(first.headers.get("access-control-allow-origin"), "*");

  const key = await e.__store.get("session:cs_test_123");
  assert.match(key, /^ENTH-/);
  const license = JSON.parse(await e.__store.get(`license:${key}`));
  assert.equal(license.session_id, "cs_test_123");
  assert.equal(license.email, "backer@example.com");
  assert.equal(license.product, "entheai-backer");
  assert.deepEqual(license.entitlements, ["beta"]);

  // Stripe retries deliveries; a replay must not mint a second key.
  assert.equal((await handleStripe(await webhook(body), e)).status, 200);
  assert.equal(await e.__store.get("session:cs_test_123"), key);
  assert.equal(e.__store.store.size, 2, "still exactly one license + one session");
});

test("webhook acknowledges but does not fulfill unpaid or unrelated events", async () => {
  const unpaid = checkoutEvent({ payment_status: "unpaid" });
  const other = JSON.stringify({ type: "invoice.paid", data: { object: { id: "in_1" } } });
  const noSession = JSON.stringify({ type: "checkout.session.completed", data: {} });
  for (const body of [unpaid, other, noSession]) {
    const e = licenseEnv();
    const res = await handleStripe(await webhook(body), e);
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { received: true });
    assert.equal(e.__store.store.size, 0, "nothing fulfilled");
  }
});

test("license verify returns entitlements for a real key and 401 for everything else", async () => {
  const e = licenseEnv();
  await handleStripe(await webhook(checkoutEvent()), e);
  const key = await e.__store.get("session:cs_test_123");

  const verify = (body) =>
    handleLicense(
      new Request("https://entheai.com/api/license/verify", { method: "POST", body }),
      e
    );

  const ok = await verify(JSON.stringify({ key }));
  assert.equal(ok.status, 200);
  assert.deepEqual(await ok.json(), {
    ok: true,
    backer: true,
    entitlements: ["beta"],
    email: "backer@example.com",
  });
  // Keys are normalized, so a pasted key with stray case/whitespace still works.
  assert.equal((await verify(JSON.stringify({ key: `  ${key.toLowerCase()}  ` }))).status, 200);

  for (const body of [
    JSON.stringify({ key: "ENTH-AAAA-AAAA-AAAA-AAAA" }),
    JSON.stringify({ key: "" }),
    JSON.stringify({ key: 42 }),
    JSON.stringify({}),
    "not json",
  ]) {
    const res = await verify(body);
    assert.equal(res.status, 401, `${body} must not verify`);
    assert.deepEqual(await res.json(), { ok: false });
  }
});

test("license verify falls back to the beta entitlement on a corrupt record", async () => {
  const e = licenseEnv();
  await e.__store.set("license:ENTH-GOOD-KEY0-AAAA-AAAA", JSON.stringify({ email: 7 }));
  await e.__store.set("license:ENTH-JUNK-KEY0-AAAA-AAAA", "{not json");
  const verify = (key) =>
    handleLicense(
      new Request("https://entheai.com/api/license/verify", {
        method: "POST",
        body: JSON.stringify({ key }),
      }),
      e
    );
  assert.deepEqual(await (await verify("ENTH-GOOD-KEY0-AAAA-AAAA")).json(), {
    ok: true,
    backer: true,
    entitlements: ["beta"],
    email: "",
  });
  assert.equal((await verify("ENTH-JUNK-KEY0-AAAA-AAAA")).status, 401);
});

test("license verify guards its bindings and method", async () => {
  const post = new Request("https://entheai.com/api/license/verify", {
    method: "POST",
    body: JSON.stringify({ key: "x" }),
  });
  assert.equal((await handleLicense(post, {})).status, 503);
  const get = new Request("https://entheai.com/api/license/verify");
  const res = await handleLicense(get, licenseEnv());
  assert.equal(res.status, 405);
  assert.equal(res.headers.get("allow"), "POST");
});

test("claim hands back the key for a fulfilled session only", async () => {
  const e = licenseEnv();
  await handleStripe(await webhook(checkoutEvent()), e);
  const key = await e.__store.get("session:cs_test_123");

  const claim = (qs) =>
    handleClaim(new Request(`https://entheai.com/api/license/claim${qs}`), e);

  const ok = await claim("?session_id=cs_test_123");
  assert.equal(ok.status, 200);
  assert.deepEqual(await ok.json(), { key });
  // Unknown or missing session_id must not leak a key.
  assert.equal((await claim("?session_id=cs_nope")).status, 404);
  assert.equal((await claim("")).status, 404);
  assert.equal((await handleClaim(new Request("https://entheai.com/api/license/claim"), {})).status, 503);
  const post = new Request("https://entheai.com/api/license/claim", { method: "POST" });
  const res = await handleClaim(post, e);
  assert.equal(res.status, 405);
  assert.equal(res.headers.get("allow"), "GET");
});

test("releases defaults to the beta channel and reports null until published", async () => {
  const e = licenseEnv();
  const releases = (qs) => handleReleases(new Request(`https://entheai.com/api/releases${qs}`), e);

  assert.deepEqual(await (await releases("")).json(), { version: null });
  await e.__store.set("releases:beta", JSON.stringify({ version: "0.2.0-beta.1" }));
  assert.deepEqual(await (await releases("")).json(), { version: "0.2.0-beta.1" });
  assert.deepEqual(await (await releases("?channel=beta")).json(), { version: "0.2.0-beta.1" });
  // An unpublished channel is null, not the beta manifest.
  assert.deepEqual(await (await releases("?channel=stable")).json(), { version: null });
  // A corrupt manifest degrades to null instead of throwing.
  await e.__store.set("releases:stable", "{not json");
  assert.deepEqual(await (await releases("?channel=stable")).json(), { version: null });

  assert.equal((await handleReleases(new Request("https://entheai.com/api/releases"), {})).status, 503);
  const post = new Request("https://entheai.com/api/releases", { method: "POST" });
  const res = await handleReleases(post, e);
  assert.equal(res.status, 405);
  assert.equal(res.headers.get("allow"), "GET");
});

test("the monetization routes reach their handlers, not the asset pipeline", async () => {
  let assetHits = 0;
  const e = {
    ...licenseEnv(),
    ASSETS: {
      async fetch() {
        assetHits++;
        return new Response("asset", { status: 200 });
      },
    },
  };
  await e.__store.set("session:cs_route", "ENTH-ROUT-EAAA-AAAA-AAAA");

  // Each route answers with its handler's own signature status/body.
  const unsigned = new Request("https://entheai.com/api/stripe/webhook", {
    method: "POST",
    body: checkoutEvent(),
  });
  assert.equal((await worker.fetch(unsigned, e)).status, 400, "webhook verified the signature");

  const verify = new Request("https://entheai.com/api/license/verify", {
    method: "POST",
    body: JSON.stringify({ key: "ENTH-AAAA-AAAA-AAAA-AAAA" }),
  });
  assert.equal((await worker.fetch(verify, e)).status, 401);

  const claim = new Request("https://entheai.com/api/license/claim?session_id=cs_route");
  assert.deepEqual(await (await worker.fetch(claim, e)).json(), {
    key: "ENTH-ROUT-EAAA-AAAA-AAAA",
  });

  assert.deepEqual(await (await worker.fetch(new Request("https://entheai.com/api/releases"), e)).json(), {
    version: null,
  });

  assert.equal(assetHits, 0, "API paths never fall through to assets");
  // Anything else still does fall through.
  assert.equal((await worker.fetch(new Request("https://entheai.com/back"), e)).status, 200);
  assert.equal(assetHits, 1);
});

test("sendBackerEmail is a no-op when unconfigured and posts to AgentMail when set", async () => {
  // No secrets → resolves without throwing and without a network call.
  await sendBackerEmail({}, "a@b.c", "ENTH-AAAA-AAAA-AAAA-AAAA");

  // With secrets → one fetch to the AgentMail send endpoint, Bearer auth, the
  // key in the plain-text body.
  let seen;
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    seen = { url, init };
    return new Response("{}", { status: 200 });
  };
  try {
    await sendBackerEmail(
      { AGENTMAIL_AGENTMAIL_API_KEY: "am_u_test", AGENTMAIL_INBOX_ID: "entheai-backer@agentmail.to" },
      "backer@example.com",
      "ENTH-KEY0-KEY0-KEY0-KEY0"
    );
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(
    seen.url,
    "https://api.agentmail.to/v0/inboxes/entheai-backer@agentmail.to/messages/send"
  );
  assert.equal(seen.init.headers.Authorization, "Bearer am_u_test");
  const body = JSON.parse(seen.init.body);
  assert.equal(body.to, "backer@example.com");
  assert.ok(body.text.includes("ENTH-KEY0-KEY0-KEY0-KEY0"), "key in the email body");
});

test("sendBackerEmail swallows network failures (best-effort delivery)", async () => {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    throw new Error("network down");
  };
  let threw = false;
  try {
    await sendBackerEmail(
      { AGENTMAIL_AGENTMAIL_API_KEY: "am_u_test", AGENTMAIL_INBOX_ID: "in@agentmail.to" },
      "a@b.c",
      "ENTH-KEY0-KEY0-KEY0-KEY0"
    );
  } catch {
    threw = true;
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(threw, false, "email failure must never propagate to the webhook");
});

// ---- Sovereign keys (constellation monetization, Lane 1) ------------------
// Ed25519-signed vkd_ tokens minted on store checkout and verified offline
// against the published public key. Records live under the `vkd:` KV prefix.

// Fixed 32-byte Ed25519 seed (bytes 0x01..0x20), base64url.
const TEST_SEED = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA";

/** Derive the base64url 32-byte public key for a seed (RFC 8410 PKCS#8). */
async function pubKeyB64FromSeed(seedB64) {
  const der = new Uint8Array([
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70,
    0x04, 0x22, 0x04, 0x20, ...b64urlDecode(seedB64),
  ]);
  const key = await crypto.subtle.importKey("pkcs8", der, { name: "Ed25519" }, true, ["sign"]);
  return (await crypto.subtle.exportKey("jwk", key)).x;
}

/** Parse the payload segment of a `vkd_sk_<payload>.<sig>` token. */
function decodePayload(token) {
  return JSON.parse(
    new TextDecoder().decode(b64urlDecode(token.slice("vkd_sk_".length).split(".")[0]))
  );
}

const sovereignVerify = (key, e) =>
  handleSovereignVerify(
    new Request("https://entheai.com/api/sovereign/verify", {
      method: "POST",
      body: JSON.stringify({ key }),
    }),
    e
  );

const sovereignCheckout = (body, overrides = {}) =>
  handleStripeCheckout(
    new Request("https://entheai.com/api/stripe/checkout", { method: "POST", body }),
    licenseEnv({ STRIPE_SECRET_KEY: "sk_test_live", ...overrides })
  );

test("sovereign: b64url round-trips bytes with no padding or URL-hostile chars", () => {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  assert.equal(b64urlDecode(b64url(bytes)).join(","), bytes.join(","));
  const enc = b64url(bytes);
  assert.ok(!enc.includes("="), "no padding");
  assert.ok(!enc.includes("+") && !enc.includes("/"), "URL-safe alphabet");
});

test("sovereign: Ed25519 sign→verify round trip over a fixed seed", async () => {
  const payload = buildSovereignPayload("roundtrip@example.com", "member");
  const token = await signSovereign(payload, TEST_SEED);
  assert.match(token, /^vkd_sk_[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/);

  const [, head, sig] = token.match(/^vkd_sk_([A-Za-z0-9_-]+)\.([A-Za-z0-9_-]+)$/);
  const payloadBytes = b64urlDecode(head);
  const sigBytes = b64urlDecode(sig);
  assert.deepEqual(JSON.parse(new TextDecoder().decode(payloadBytes)), payload);

  const pub = await crypto.subtle.importKey(
    "jwk",
    { kty: "OKP", crv: "Ed25519", x: await pubKeyB64FromSeed(TEST_SEED) },
    { name: "Ed25519" },
    false,
    ["verify"]
  );
  assert.equal(await crypto.subtle.verify("Ed25519", pub, sigBytes, payloadBytes), true);
  const tampered = Uint8Array.from(sigBytes);
  tampered[0] ^= 0xff;
  assert.equal(await crypto.subtle.verify("Ed25519", pub, tampered, payloadBytes), false);
});

test("sovereign: payload carries the tier matrix (TTLs + entitlements)", () => {
  const expected = {
    presence: { ttl: 30 * 86400, ent: ["community"] },
    backer: { ttl: 365 * 86400, ent: ["community", "gallery-hd", "quantal", "beta"] },
    member: {
      ttl: 5 * 365 * 86400,
      ent: ["community", "gallery-hd", "quantal", "labs", "quant-api", "beta"],
    },
  };
  for (const [tier, { ttl, ent }] of Object.entries(expected)) {
    const p = buildSovereignPayload("tier@example.com", tier);
    assert.equal(p.v, 1);
    assert.equal(p.iss, "vaked-sovereign-worker");
    assert.equal(p.tier, tier);
    assert.deepEqual(p.ent, ent);
    assert.equal(p.exp - p.iat, ttl, `${tier} TTL`);
    assert.ok(p.iat <= Math.floor(Date.now() / 1000), "iat is in the past");
  }
  // Unknown tiers degrade to presence, never throw.
  const fallback = buildSovereignPayload("x@y.z", "hyperbacker");
  assert.equal(fallback.tier, "hyperbacker");
  assert.deepEqual(fallback.ent, TIER_ENTITLEMENTS.presence);
});

test("sovereign: both public key paths serve the hardcoded 32-byte key", async () => {
  for (const path of ["/.well-known/sovereign.pub", "/api/sovereign/pub"]) {
    const res = await worker.fetch(new Request(`https://entheai.com${path}`), {});
    assert.equal(res.status, 200, path);
    assert.equal(res.headers.get("content-type"), "application/octet-stream", path);
    const bytes = new Uint8Array(await res.arrayBuffer());
    assert.equal(bytes.length, 32, path);
    assert.equal(b64url(bytes), SOVEREIGN_PUBLIC_KEY_B64URL, path);
  }
});

test("sovereign: public key endpoint degrades to 503 on an unconfigured key", async () => {
  const res = await handleSovereignPub(
    new Request("https://entheai.com/.well-known/sovereign.pub"),
    { SOVEREIGN_PUBLIC_KEY: "not-base64url!!!" }
  );
  assert.equal(res.status, 503);
  assert.deepEqual(await res.json(), { error: "public key unconfigured" });
});

test("sovereign: verify accepts a store-minted member token (signature + exp + KV record)", async () => {
  const e = licenseEnv({
    SOVEREIGN_SIGNING_KEY: TEST_SEED,
    SOVEREIGN_PUBLIC_KEY: await pubKeyB64FromSeed(TEST_SEED),
  });
  await handleStripe(await webhook(checkoutEvent({ metadata: { tier: "member" } })), e);

  const token = await e.__store.get("vkd:license:backer@example.com");
  assert.ok(token && token.startsWith("vkd_sk_"), "token minted into KV under vkd:license:<email>");
  const payload = decodePayload(token);
  assert.equal(payload.sub, "backer@example.com");
  assert.equal(payload.tier, "member");

  const ok = await sovereignVerify(token, e);
  assert.equal(ok.status, 200);
  assert.deepEqual(await ok.json(), {
    ok: true,
    tier: "member",
    ent: TIER_ENTITLEMENTS.member,
    sub: "backer@example.com",
    exp: payload.exp,
  });
});

test("sovereign: verify rejects a tampered signature and an expired token", async () => {
  const e = licenseEnv({
    SOVEREIGN_SIGNING_KEY: TEST_SEED,
    SOVEREIGN_PUBLIC_KEY: await pubKeyB64FromSeed(TEST_SEED),
  });
  await handleStripe(await webhook(checkoutEvent({ metadata: { tier: "backer" } })), e);
  const token = await e.__store.get("vkd:license:backer@example.com");

  // Same sub with a valid KV record: rejection provably comes from the crypto
  // path, not the record cross-check.
  const dot = token.lastIndexOf(".");
  const flip = token[dot + 1] === "A" ? "B" : "A";
  const tampered = token.slice(0, dot + 1) + flip + token.slice(dot + 2);
  const bad = await sovereignVerify(tampered, e);
  assert.equal(bad.status, 401);
  assert.deepEqual(await bad.json(), { ok: false });

  // Signature valid but exp in the past — no KV store bound, so the rejection
  // provably comes from the expiry check.
  const expired = await signSovereign(
    buildSovereignPayload("expired@example.com", "backer", Date.now() - 400 * 86400 * 1000),
    TEST_SEED
  );
  const offline = await sovereignVerify(expired, {
    SOVEREIGN_PUBLIC_KEY: await pubKeyB64FromSeed(TEST_SEED),
  });
  assert.equal(offline.status, 401);
  assert.deepEqual(await offline.json(), { ok: false });
});

test("sovereign: verify is offline-friendly and treats a missing KV record as revocation", async () => {
  const pubB64 = await pubKeyB64FromSeed(TEST_SEED);
  const fresh = await signSovereign(buildSovereignPayload("offline@example.com", "backer"), TEST_SEED);

  // No KV store bound → signature + exp is the whole story.
  const offline = await sovereignVerify(fresh, { SOVEREIGN_PUBLIC_KEY: pubB64 });
  assert.equal(offline.status, 200);
  assert.equal((await offline.json()).ok, true);

  // Store bound but no vkd:license:<sub> record → revoked.
  const revoked = await sovereignVerify(fresh, licenseEnv({ SOVEREIGN_PUBLIC_KEY: pubB64 }));
  assert.equal(revoked.status, 401);
  assert.deepEqual(await revoked.json(), { ok: false });
});

test("sovereign: verify guards its method and rejects malformed keys", async () => {
  const e = licenseEnv({ SOVEREIGN_PUBLIC_KEY: SOVEREIGN_PUBLIC_KEY_B64URL });
  const get = new Request("https://entheai.com/api/sovereign/verify");
  const res = await handleSovereignVerify(get, e);
  assert.equal(res.status, 405);
  assert.equal(res.headers.get("allow"), "POST");
  const cases = ["", "not-a-sovereign-key", "vkd_sk_AAAA.BBB", "   ", JSON.stringify({ key: 42 })];
  for (const key of cases) {
    const r = await sovereignVerify(key, e);
    assert.equal(r.status, 401, `${key} must not verify`);
    assert.deepEqual(await r.json(), { ok: false });
  }
  for (const body of ["not json", JSON.stringify({}), JSON.stringify({ key: "" })]) {
    const r = await handleSovereignVerify(
      new Request("https://entheai.com/api/sovereign/verify", { method: "POST", body }),
      e
    );
    assert.equal(r.status, 401, `${body} must not verify`);
    assert.deepEqual(await r.json(), { ok: false });
  }
});

test("sovereign: checkout returns 503 without STRIPE_SECRET_KEY", async () => {
  const res = await handleStripeCheckout(
    new Request("https://entheai.com/api/stripe/checkout", {
      method: "POST",
      body: JSON.stringify({ items: [{ sku: "tshirt", qty: 1 }] }),
    }),
    licenseEnv({ STRIPE_SECRET_KEY: undefined })
  );
  assert.equal(res.status, 503);
  assert.deepEqual(await res.json(), { error: "stripe not configured" });
});

test("sovereign: checkout POSTs a hosted session to Stripe with catalog prices, tier metadata, and overrides", async () => {
  let seen;
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    seen = { url, init };
    return new Response(JSON.stringify({ url: "https://checkout.stripe.com/c/pay/cs_test_shop" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  try {
    const res = await sovereignCheckout(
      JSON.stringify({
        items: [
          { sku: "hoodie", qty: 2 },
          { sku: "vinyl", qty: 1, unit_amount: 4444 },
          { sku: "vakedide-supporter", qty: 1 },
        ],
      })
    );
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { url: "https://checkout.stripe.com/c/pay/cs_test_shop" });
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(seen.url, "https://api.stripe.com/v1/checkout/sessions");
  assert.equal(seen.init.method, "POST");
  assert.equal(seen.init.headers.Authorization, "Bearer sk_test_live");
  assert.equal(seen.init.headers["Content-Type"], "application/x-www-form-urlencoded");
  const form = new URLSearchParams(seen.init.body);
  assert.equal(form.get("mode"), "payment");
  assert.equal(form.get("success_url"), "https://store.vaked.dev/?checkout={CHECKOUT_SESSION_ID}");
  assert.equal(form.get("cancel_url"), "https://store.vaked.dev/?canceled=1");
  assert.equal(form.get("metadata[source]"), "store.vaked.dev");
  assert.equal(form.get("managed_payments[enabled]"), "false", "Managed Payments off (no tax_code for inline SKUs)");
  assert.equal(form.get("metadata[tier]"), "member", "vakedide sku upgrades the tier");
  assert.equal(form.get("line_items[0][quantity]"), "2");
  assert.equal(form.get("line_items[0][price_data][currency]"), "eur");
  assert.equal(form.get("line_items[0][price_data][product_data][name]"), "hoodie");
  assert.equal(form.get("line_items[0][price_data][unit_amount]"), String(STORE_CATALOG.hoodie));
  assert.equal(form.get("line_items[1][price_data][unit_amount]"), "4444", "override wins over catalog");
  assert.equal(
    form.get("line_items[2][price_data][unit_amount]"),
    String(STORE_CATALOG["vakedide-supporter"])
  );
});

test("sovereign: checkout defaults to backer, honors explicit tiers, and defaults quantities", async () => {
  const seen = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    seen.push(new URLSearchParams(init.body));
    return new Response(JSON.stringify({ url: "https://checkout.stripe.com/c/pay/cs_x" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  try {
    assert.equal((await sovereignCheckout(JSON.stringify({ items: [{ sku: "tshirt" }] }))).status, 200);
    assert.equal(
      (await sovereignCheckout(JSON.stringify({ items: [{ sku: "scarf" }], tier: "presence" }))).status,
      200
    );
    assert.equal(
      (await sovereignCheckout(JSON.stringify({ items: [{ sku: "tshirt", qty: -3 }], tier: "member" }))).status,
      200
    );
    // Case-insensitive member detection on the sku itself.
    assert.equal(
      (await sovereignCheckout(JSON.stringify({ items: [{ sku: "VAKEDIDE-SUPPORTER" }] }))).status,
      200
    );
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(seen[0].get("metadata[tier]"), "backer", "default tier is backer");
  assert.equal(seen[0].get("line_items[0][price_data][unit_amount]"), "3500");
  assert.equal(seen[0].get("line_items[0][quantity]"), "1", "quantity defaults to 1");
  assert.equal(seen[1].get("metadata[tier]"), "presence", "explicit tier is honored");
  assert.equal(seen[2].get("metadata[tier]"), "member", "explicit tier is honored");
  assert.equal(seen[2].get("line_items[0][quantity]"), "1", "invalid quantity defaults to 1");
  assert.equal(seen[3].get("metadata[tier]"), "member", "sku match is case-insensitive");
});

test("sovereign: checkout validates items, skus, and overrides locally without calling Stripe", async () => {
  let calls = 0;
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    calls++;
    throw new Error("must not be reached");
  };
  try {
    const cases = [
      [JSON.stringify({ items: [] }), "items required"],
      [JSON.stringify({}), "items required"],
      [JSON.stringify({ items: [{ qty: 1 }] }), "item sku required"],
      [JSON.stringify({ items: [{ sku: "quantum-teapot" }] }), "unknown sku: quantum-teapot"],
      [JSON.stringify({ items: [{ sku: "tshirt", unit_amount: -5 }] }), "invalid unit_amount for tshirt"],
      [JSON.stringify({ items: [{ sku: "tshirt", unit_amount: 1.5 }] }), "invalid unit_amount for tshirt"],
      ["not json", "body must be JSON"],
    ];
    for (const [body, message] of cases) {
      const res = await sovereignCheckout(body);
      assert.equal(res.status, 400, `${body} must be rejected`);
      assert.deepEqual(await res.json(), { error: message });
    }
    const get = new Request("https://entheai.com/api/stripe/checkout");
    const res = await handleStripeCheckout(get, licenseEnv({ STRIPE_SECRET_KEY: "sk_test_live" }));
    assert.equal(res.status, 405);
    assert.equal(res.headers.get("allow"), "POST");
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(calls, 0, "validation failures never reach Stripe");
});

test("sovereign: checkout surfaces Stripe errors and upstream failures", async () => {
  let mode = "error";
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    if (mode === "error") {
      return new Response(JSON.stringify({ error: { message: "no such coupon" } }), {
        status: 400,
        headers: { "content-type": "application/json" },
      });
    }
    if (mode === "nourl") {
      return new Response(JSON.stringify({}), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }
    throw new Error("connection refused");
  };
  try {
    const err = await sovereignCheckout(JSON.stringify({ items: [{ sku: "tshirt" }] }));
    assert.equal(err.status, 400);
    assert.deepEqual(await err.json(), { error: "no such coupon" });

    mode = "nourl";
    const noUrl = await sovereignCheckout(JSON.stringify({ items: [{ sku: "tshirt" }] }));
    assert.equal(noUrl.status, 502);
    assert.deepEqual(await noUrl.json(), { error: "stripe returned no url" });

    mode = "down";
    const down = await sovereignCheckout(JSON.stringify({ items: [{ sku: "tshirt" }] }));
    assert.equal(down.status, 502);
    assert.deepEqual(await down.json(), { error: "stripe unreachable" });
  } finally {
    globalThis.fetch = realFetch;
  }
});

test("sovereign: webhook mints a vkd token when metadata.tier is member and never when absent", async () => {
  const e = licenseEnv({ SOVEREIGN_SIGNING_KEY: TEST_SEED });
  const res = await handleStripe(
    await webhook(checkoutEvent({ id: "cs_member_1", metadata: { tier: "member" } })),
    e
  );
  assert.equal(res.status, 200);
  const token = await e.__store.get("vkd:license:backer@example.com");
  assert.ok(token && token.startsWith("vkd_sk_"), "sovereign token minted under vkd:license:<email>");
  const payload = decodePayload(token);
  assert.equal(payload.sub, "backer@example.com");
  assert.equal(payload.tier, "member");
  assert.deepEqual(payload.ent, TIER_ENTITLEMENTS.member);
  assert.equal(payload.exp - payload.iat, SOVEREIGN_TTL_SECS.member);
  // The entheai license path is untouched: still one ENTH- license + session.
  assert.match(await e.__store.get("session:cs_member_1"), /^ENTH-/);

  // No metadata.tier → no sovereign keys at all.
  const plain = licenseEnv({ SOVEREIGN_SIGNING_KEY: TEST_SEED });
  await handleStripe(await webhook(checkoutEvent({ id: "cs_plain_1" })), plain);
  for (const k of plain.__store.store.keys()) {
    assert.ok(!k.startsWith("vkd:"), `default path must not mint vkd keys (saw ${k})`);
  }
});

test("sovereign: webhook mints per-tier entitlements and honors sub fallbacks", async () => {
  const cases = [
    { tier: "presence", ent: TIER_ENTITLEMENTS.presence, ttl: SOVEREIGN_TTL_SECS.presence },
    { tier: "backer", ent: TIER_ENTITLEMENTS.backer, ttl: SOVEREIGN_TTL_SECS.backer },
  ];
  for (const [i, c] of cases.entries()) {
    const e = licenseEnv({ SOVEREIGN_SIGNING_KEY: TEST_SEED });
    await handleStripe(
      await webhook(checkoutEvent({ id: `cs_tier_${i}`, metadata: { tier: c.tier } })),
      e
    );
    const payload = decodePayload(await e.__store.get("vkd:license:backer@example.com"));
    assert.equal(payload.tier, c.tier);
    assert.deepEqual(payload.ent, c.ent);
    assert.equal(payload.exp - payload.iat, c.ttl);
  }

  // email absent → metadata.sub; neither → session id.
  const subFallback = licenseEnv({ SOVEREIGN_SIGNING_KEY: TEST_SEED });
  await handleStripe(
    await webhook(
      checkoutEvent({ id: "cs_sub_1", customer_details: {}, metadata: { tier: "backer", sub: "wallet-abc" } })
    ),
    subFallback
  );
  assert.ok(await subFallback.__store.get("vkd:license:wallet-abc"), "metadata.sub fallback");

  const idFallback = licenseEnv({ SOVEREIGN_SIGNING_KEY: TEST_SEED });
  await handleStripe(
    await webhook(checkoutEvent({ id: "cs_id_1", customer_details: {}, metadata: { tier: "backer" } })),
    idFallback
  );
  assert.ok(await idFallback.__store.get("vkd:license:cs_id_1"), "session id fallback");
});

test("sovereign: webhook skips the mint for entheai-backer and unconfigured signing keys", async () => {
  const e = licenseEnv({ SOVEREIGN_SIGNING_KEY: TEST_SEED });
  await handleStripe(
    await webhook(checkoutEvent({ id: "cs_enth_1", metadata: { tier: "entheai-backer" } })),
    e
  );
  for (const k of e.__store.store.keys()) {
    assert.ok(!k.startsWith("vkd:"), `entheai-backer must not mint sovereign keys (saw ${k})`);
  }

  const unconfigured = licenseEnv();
  const res = await handleStripe(
    await webhook(checkoutEvent({ id: "cs_nokey_1", metadata: { tier: "member" } })),
    unconfigured
  );
  assert.equal(res.status, 200, "missing signing key never fails the webhook");
  for (const k of unconfigured.__store.store.keys()) {
    assert.ok(!k.startsWith("vkd:"), `unconfigured must not mint sovereign keys (saw ${k})`);
  }
});

test("sovereign: the sovereign mint is idempotent per session like the entheai one", async () => {
  const e = licenseEnv({ SOVEREIGN_SIGNING_KEY: TEST_SEED });
  const body = checkoutEvent({ id: "cs_replay_1", metadata: { tier: "member" } });
  assert.equal((await handleStripe(await webhook(body), e)).status, 200);
  const token = await e.__store.get("vkd:license:backer@example.com");
  assert.equal(e.__store.store.size, 3, "license + session + vkd");
  assert.equal((await handleStripe(await webhook(body), e)).status, 200);
  assert.equal(await e.__store.get("vkd:license:backer@example.com"), token, "no second mint on replay");
  assert.equal(e.__store.store.size, 3, "replay adds nothing");
});

test("sovereign: an end-to-end store purchase verifies on the verify endpoint", async () => {
  const pubB64 = await pubKeyB64FromSeed(TEST_SEED);
  const e = licenseEnv({ SOVEREIGN_SIGNING_KEY: TEST_SEED, SOVEREIGN_PUBLIC_KEY: pubB64 });
  await handleStripe(
    await webhook(checkoutEvent({ id: "cs_e2e_1", metadata: { tier: "presence" } })),
    e
  );
  const token = await e.__store.get("vkd:license:backer@example.com");
  const ok = await sovereignVerify(token, e);
  assert.equal(ok.status, 200);
  assert.deepEqual(await ok.json(), {
    ok: true,
    tier: "presence",
    ent: TIER_ENTITLEMENTS.presence,
    sub: "backer@example.com",
    exp: decodePayload(token).exp,
  });
});

test("the sovereign routes reach their handlers, not the asset pipeline", async () => {
  let assetHits = 0;
  const e = {
    ...licenseEnv({ STRIPE_SECRET_KEY: undefined, SOVEREIGN_SIGNING_KEY: TEST_SEED }),
    ASSETS: {
      async fetch() {
        assetHits++;
        return new Response("asset", { status: 200 });
      },
    },
  };
  const checkout = new Request("https://entheai.com/api/stripe/checkout", {
    method: "POST",
    body: JSON.stringify({ items: [{ sku: "tshirt" }] }),
  });
  assert.equal((await worker.fetch(checkout, e)).status, 503, "checkout reached its handler (no secret)");
  const verifyReq = new Request("https://entheai.com/api/sovereign/verify", {
    method: "POST",
    body: JSON.stringify({ key: "vkd_sk_bogus" }),
  });
  assert.equal((await worker.fetch(verifyReq, e)).status, 401, "verify reached its handler");
  assert.equal(
    (await worker.fetch(new Request("https://entheai.com/.well-known/sovereign.pub"), e)).status,
    200
  );
  assert.equal((await worker.fetch(new Request("https://entheai.com/api/sovereign/pub"), e)).status, 200);
  assert.equal(assetHits, 0, "sovereign API paths never fall through to assets");
});

// ---- Redis gateway (REDIS_GATEWAY_URL / REDIS_GATEWAY_TOKEN) --------------
// The license store prefers the HTTPS gateway over the legacy TCP path when
// both gateway env vars are set, and still answers 503 when neither a store,
// the gateway pair, nor REDIS_PUBLIC_URL is configured. The gateway path is
// exercised against a stubbed fetch so no socket or real network is touched.

/** Replace globalThis.fetch for the duration of one test. */
async function withFetch(mock, fn) {
  const realFetch = globalThis.fetch;
  globalThis.fetch = mock;
  try {
    return await fn();
  } finally {
    globalThis.fetch = realFetch;
  }
}

/** A fetch stub that records (url, init) calls and answers from `routes`. */
function fetchMock(routes, log = []) {
  return async (url, init) => {
    log.push({ url: String(url), init });
    const hit = routes.find((r) => r.test && r.test.test(String(url)));
    if (!hit) return new Response(JSON.stringify({ error: "unmocked" }), { status: 501 });
    if (hit.status === 404) return new Response(JSON.stringify({ error: "not found" }), { status: 404 });
    if (hit.status) return new Response(JSON.stringify(hit.body ?? {}), { status: hit.status });
    return new Response(JSON.stringify(hit.body ?? {}), { status: 200 });
  };
}

test("licenseStore prefers the Redis gateway when both gateway vars are set", async () => {
  const calls = [];
  await withFetch(
    fetchMock([{ test: /\/session%3Acs_gw/, body: { value: "ENTH-GATEWAY-AAAA-AAAA" } }], calls),
    async () => {
      const e = licenseEnv({
        __store: undefined,
        REDIS_PUBLIC_URL: undefined,
        REDIS_GATEWAY_URL: "https://gw.up.railway.app",
        REDIS_GATEWAY_TOKEN: "gw-token",
      });
      const req = new Request("https://entheai.com/api/license/claim?session_id=cs_gw");
      const res = await handleClaim(req, e);
      assert.equal(res.status, 200);
      assert.deepEqual(await res.json(), { key: "ENTH-GATEWAY-AAAA-AAAA" });
    }
  );
  assert.equal(calls.length, 1, "exactly one gateway call");
  assert.equal(calls[0].url, "https://gw.up.railway.app/session%3Acs_gw");
  assert.equal(calls[0].init.headers.Authorization, "Bearer gw-token");
});

test("licenseStore ignores REDIS_PUBLIC_URL once the gateway pair is set", async () => {
  const calls = [];
  await withFetch(
    fetchMock(
      [{ test: /\/license%3AENTH-GW/, body: { value: JSON.stringify({ entitlements: ["beta"], email: "gw@example.com" }) } }],
      calls
    ),
    async () => {
      const e = licenseEnv({
        __store: undefined,
        REDIS_PUBLIC_URL: "redis://default:pw@host.proxy.rlwy.net:6379",
        REDIS_GATEWAY_URL: "https://gw.up.railway.app",
        REDIS_GATEWAY_TOKEN: "gw-token",
      });
      // handleLicense calls get("license:ENTH-GW") — must go over the gateway.
      const req = new Request("https://entheai.com/api/license/verify", {
        method: "POST",
        body: JSON.stringify({ key: "enth-gw" }),
      });
      const res = await handleLicense(req, e);
      assert.equal(res.status, 200);
    }
  );
  assert.equal(calls.length, 1, "only the gateway is consulted, never the TCP URL");
  assert.equal(calls[0].url, "https://gw.up.railway.app/license%3AENTH-GW");
});

test("licenseStore needs BOTH gateway vars; partial config falls back to 503", async () => {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    throw new Error("fetch must not run for partial gateway config");
  };
  try {
    const urlOnly = licenseEnv({
      __store: undefined,
      REDIS_PUBLIC_URL: undefined,
      REDIS_GATEWAY_URL: "https://gw.up.railway.app",
    });
    assert.equal(
      (await handleClaim(new Request("https://entheai.com/api/license/claim?session_id=cs_1"), urlOnly)).status,
      503
    );

    const tokenOnly = licenseEnv({
      __store: undefined,
      REDIS_PUBLIC_URL: undefined,
      REDIS_GATEWAY_TOKEN: "gw-token",
    });
    assert.equal(
      (await handleClaim(new Request("https://entheai.com/api/license/claim?session_id=cs_1"), tokenOnly)).status,
      503
    );
  } finally {
    globalThis.fetch = realFetch;
  }
});

test("redisHttp PUT/DELETE hit the gateway with the raw body and bearer token", async () => {
  const calls = [];
  const result = await withFetch(
    fetchMock([{ test: /\/license%3AENTH-W/, body: { ok: true } }], calls),
    () =>
      Promise.all([
        redisHttp("https://gw.up.railway.app/", "tok", "PUT", "license:ENTH-W", "raw-value"),
        redisHttp("https://gw.up.railway.app", "tok", "DELETE", "license:ENTH-W"),
      ])
  );
  assert.deepEqual(result, [true, true]);
  assert.equal(calls.length, 2);
  const put = calls[0];
  assert.equal(put.url, "https://gw.up.railway.app/license%3AENTH-W");
  assert.equal(put.init.method, "PUT");
  assert.equal(put.init.body, "raw-value");
  assert.equal(put.init.headers["Content-Type"], "text/plain");
  assert.equal(put.init.headers.Authorization, "Bearer tok");
  const del = calls[1];
  assert.equal(del.init.method, "DELETE");
  assert.equal(del.init.body, undefined);
  assert.equal(del.url, "https://gw.up.railway.app/license%3AENTH-W", "trailing slash normalized");
});
