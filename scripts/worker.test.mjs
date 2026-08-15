import test from "node:test";
import assert from "node:assert/strict";
import worker, {
  handleEntropy,
  handleStripe,
  handleLicense,
  handleClaim,
  handleReleases,
  generateLicenseKey,
  sendBackerEmail,
  LICENSE_ALPHABET,
  SCHEMA,
  KV_KEY,
  STALE_AFTER_MS,
} from "../src/worker.mjs";

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
