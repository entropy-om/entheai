// gateway/test/gateway.test.mjs — unit tests for the gateway handler against a
// fake in-memory store, plus a real-HTTP smoke test through startServer().
// Run: node --test test/   (or npm test from gateway/)

import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { createHandler, createRateLimiter } from "../handler.mjs";
import { startServer } from "../server.mjs";

/** In-memory store double matching the handler's { get, set, del, ping } surface. */
function fakeStore(initial = {}) {
  const map = new Map(Object.entries(initial));
  return {
    map,
    async get(k) {
      return map.has(k) ? map.get(k) : null;
    },
    async set(k, v) {
      map.set(k, v);
    },
    async del(k) {
      map.delete(k);
    },
    async ping() {
      return "PONG";
    },
  };
}

const TOKEN = "test-token";

function make(token = TOKEN) {
  const store = fakeStore();
  return { store, handle: createHandler({ store, token }) };
}

function auth(token = TOKEN) {
  return { authorization: `Bearer ${token}` };
}

async function body(res) {
  return JSON.parse(res.body);
}

// ---- auth ----------------------------------------------------------------

test("401 without a token, with a wrong token, or with a malformed header", async () => {
  const { handle } = make();
  const cases = [
    {},
    { authorization: "" },
    { authorization: "Basic abc" },
    { authorization: `Bearer ${TOKEN}x` }, // length matches only with the real token
    { authorization: `Bearer wrong-token` },
  ];
  for (const headers of cases) {
    for (const [m, p] of [["GET", "/health"], ["GET", "/license%3AENTH-1234"], ["PUT", "/k"], ["DELETE", "/k"]]) {
      const res = await handle(m, p, headers, "x");
      assert.equal(res.status, 401, `${m} ${p} with ${JSON.stringify(headers)}`);
      assert.deepEqual(await body(res), { error: "unauthorized" });
    }
  }
});

test("the right token passes on every method", async () => {
  const { store, handle } = make();
  await store.set("license:ENTH-OK", "yes");
  assert.equal((await handle("GET", "/license%3AENTH-OK", auth())).status, 200);
  assert.equal((await handle("PUT", "/k", auth(), "v")).status, 200);
  assert.equal((await handle("DELETE", "/k", auth())).status, 200);
  assert.equal((await handle("GET", "/health", auth())).status, 200);
});

// ---- GET -----------------------------------------------------------------

test("GET returns 200 {value} for a present key, 404 for a missing one", async () => {
  const { store, handle } = make();
  await store.set("license:ENTH-1234-ABCD", "the-license");
  const hit = await handle("GET", "/license%3AENTH-1234-ABCD", auth());
  assert.equal(hit.status, 200);
  assert.deepEqual(await body(hit), { value: "the-license" });

  const miss = await handle("GET", "/license%3AENTH-NOPE", auth());
  assert.equal(miss.status, 404);
  assert.deepEqual(await body(miss), { error: "not found" });
});

test("GET handles plain (unencoded) keys and dot/colon keys", async () => {
  const { store, handle } = make();
  await store.set("releases:beta", "{}");
  assert.equal((await handle("GET", "/releases%3Abeta", auth())).status, 200);
  assert.equal((await handle("GET", "/releases:beta", auth())).status, 200); // raw colon tolerated
});

// ---- PUT -----------------------------------------------------------------

test("PUT stores the raw body; a subsequent GET round-trips it", async () => {
  const { store, handle } = make();
  const res = await handle("PUT", "/license%3AENTH-ROUND", auth(), '{"session":"cs_1"}');
  assert.equal(res.status, 200);
  assert.deepEqual(await body(res), { ok: true });
  assert.equal(store.map.get("license:ENTH-ROUND"), '{"session":"cs_1"}');

  const hit = await handle("GET", "/license%3AENTH-ROUND", auth());
  assert.deepEqual(await body(hit), { value: '{"session":"cs_1"}' });
});

test("PUT stores an empty body as an empty string", async () => {
  const { store, handle } = make();
  assert.equal((await handle("PUT", "/empty", auth(), "")).status, 200);
  assert.equal(store.map.get("empty"), "");
});

// ---- DELETE --------------------------------------------------------------

test("DELETE removes the key; GET afterwards is 404", async () => {
  const { store, handle } = make();
  await store.set("session:cs_old", "ENTH-GONE");
  const res = await handle("DELETE", "/session%3Acs_old", auth());
  assert.equal(res.status, 200);
  assert.deepEqual(await body(res), { ok: true });
  assert.equal(store.map.has("session:cs_old"), false);
  assert.equal((await handle("GET", "/session%3Acs_old", auth())).status, 404);
});

test("DELETE on a missing key still returns 200 {ok:true}", async () => {
  const { handle } = make();
  const res = await handle("DELETE", "/never-existed", auth());
  assert.equal(res.status, 200);
  assert.deepEqual(await body(res), { ok: true });
});

// ---- health --------------------------------------------------------------

test("GET /health pings Redis and returns 200 {ok:true}", async () => {
  const { handle } = make();
  const res = await handle("GET", "/health", auth());
  assert.equal(res.status, 200);
  assert.deepEqual(await body(res), { ok: true });
});

test("GET /health returns 503 when the store ping fails", async () => {
  const store = fakeStore();
  store.ping = async () => {
    throw new Error("redis: connection refused");
  };
  const handle = createHandler({ store, token: TOKEN });
  const res = await handle("GET", "/health", auth());
  assert.equal(res.status, 503);
  assert.deepEqual(await body(res), { error: "redis unreachable" });
});

// ---- methods / misc ------------------------------------------------------

test("unsupported methods get 405 with an Allow header", async () => {
  const { handle } = make();
  const post = await handle("POST", "/some-key", auth(), "x");
  assert.equal(post.status, 405);
  assert.equal(post.headers.allow, "GET, PUT, DELETE");
  const patch = await handle("PATCH", "/health", auth());
  assert.equal(patch.status, 405);
  assert.equal(patch.headers.allow, "GET");
});

test("malformed percent-encoding is a 400", async () => {
  const { handle } = make();
  const res = await handle("GET", "/%ZZ-not-encoded", auth());
  assert.equal(res.status, 400);
  assert.deepEqual(await body(res), { error: "invalid key encoding" });
});

test("all responses are application/json", async () => {
  const { handle } = make();
  for (const res of [
    await handle("GET", "/nope", auth()),
    await handle("PUT", "/k", auth(), "v"),
    await handle("DELETE", "/k", auth()),
    await handle("GET", "/health", auth()),
    await handle("GET", "/", {}),
  ]) {
    assert.equal(res.headers["content-type"], "application/json");
  }
});

// ---- real HTTP through startServer ---------------------------------------

test("end-to-end over node:http: auth + PUT/GET/DELETE + health", async (t) => {
  const store = fakeStore();
  const server = startServer({ store, token: TOKEN, port: 0, host: "127.0.0.1" });
  t.after(() => server.close());
  await new Promise((r) => server.once("listening", r));
  const base = `http://127.0.0.1:${server.address().port}`;

  // Unauthenticated request is rejected at the HTTP layer too.
  const noAuth = await fetch(`${base}/health`);
  assert.equal(noAuth.status, 401);

  const h = { authorization: `Bearer ${TOKEN}` };

  const health = await fetch(`${base}/health`, { headers: h });
  assert.equal(health.status, 200);
  assert.deepEqual(await health.json(), { ok: true });

  const put = await fetch(`${base}/license%3AENTH-E2E`, {
    method: "PUT",
    headers: { ...h, "content-type": "text/plain" },
    body: "e2e-value",
  });
  assert.equal(put.status, 200);

  const get = await fetch(`${base}/license%3AENTH-E2E`, { headers: h });
  assert.equal(get.status, 200);
  assert.deepEqual(await get.json(), { value: "e2e-value" });

  const del = await fetch(`${base}/license%3AENTH-E2E`, { method: "DELETE", headers: h });
  assert.equal(del.status, 200);
  assert.equal((await fetch(`${base}/license%3AENTH-E2E`, { headers: h })).status, 404);
});

// ---- failed-auth rate limiting + body cap + bootstrap guard ----------------

test("failed-auth limiter: 5 failures allowed, the 6th is blocked, success clears", () => {
  const rl = createRateLimiter({ windowMs: 10_000, max: 5 });
  for (let i = 0; i < 5; i++) {
    assert.equal(rl.isBlocked("1.2.3.4"), false, `attempt ${i + 1} not blocked`);
    rl.recordFailure("1.2.3.4");
  }
  assert.equal(rl.isBlocked("1.2.3.4"), true, "6th attempt blocked");
  // Other IPs are untouched; remainingMs reports the window.
  assert.equal(rl.isBlocked("5.6.7.8"), false);
  assert.ok(rl.remainingMs("1.2.3.4") > 0 && rl.remainingMs("1.2.3.4") <= 10_000);
  // A successful auth clears the budget.
  rl.clear("1.2.3.4");
  assert.equal(rl.isBlocked("1.2.3.4"), false);
});

test("over-HTTP: 5 failed auths then 429 until the window rolls over", async (t) => {
  const server = startServer({
    store: fakeStore(),
    token: TOKEN,
    port: 0,
    host: "127.0.0.1",
    rateLimit: { windowMs: 150, max: 5 },
  });
  t.after(() => server.close());
  await new Promise((r) => server.once("listening", r));
  const base = `http://127.0.0.1:${server.address().port}`;

  const statuses = [];
  for (let i = 0; i < 6; i++) statuses.push((await fetch(`${base}/health`)).status);
  assert.deepEqual(statuses, [401, 401, 401, 401, 401, 429]);

  // The block is per-IP and enforced before auth, so even a valid token is
  // gated while the budget is exhausted.
  assert.equal((await fetch(`${base}/health`, { headers: auth() })).status, 429);
  assert.equal((await fetch(`${base}/health`, { headers: auth() })).status, 429);

  // Once the window rolls over the budget is fresh and authed requests work.
  await new Promise((r) => setTimeout(r, 200));
  assert.equal((await fetch(`${base}/health`, { headers: auth() })).status, 200);
});

test("over-HTTP: failed-auth buckets are keyed per X-Forwarded-For client", async (t) => {
  const server = startServer({
    store: fakeStore(),
    token: TOKEN,
    port: 0,
    host: "127.0.0.1",
    rateLimit: { windowMs: 150, max: 5 },
  });
  t.after(() => server.close());
  await new Promise((r) => server.once("listening", r));
  const base = `http://127.0.0.1:${server.address().port}`;

  // Client A exhausts its own budget…
  for (let i = 0; i < 5; i++) {
    assert.equal(
      (await fetch(`${base}/health`, { headers: { "x-forwarded-for": "1.2.3.4" } })).status,
      401
    );
  }
  // …while client B (same TCP peer behind the LB) is untouched.
  assert.equal(
    (await fetch(`${base}/health`, { headers: { "x-forwarded-for": "5.6.7.8" } })).status,
    401
  );
  // And A is now blocked at the door.
  assert.equal(
    (await fetch(`${base}/health`, { headers: { "x-forwarded-for": "1.2.3.4" } })).status,
    429
  );
});

test("over-HTTP: PUT bodies over the cap are rejected with 413 and never stored", async (t) => {
  const store = fakeStore();
  const server = startServer({
    store,
    token: TOKEN,
    port: 0,
    host: "127.0.0.1",
    maxBodyBytes: 64,
  });
  t.after(() => server.close());
  await new Promise((r) => server.once("listening", r));
  const base = `http://127.0.0.1:${server.address().port}`;

  const big = await fetch(`${base}/license%3AENTH-BIG`, {
    method: "PUT",
    headers: auth(),
    body: "x".repeat(65),
  });
  assert.equal(big.status, 413);
  assert.deepEqual(await big.json(), { error: "body too large" });
  assert.equal(store.map.has("license:ENTH-BIG"), false, "oversized body never stored");

  // At the cap or under it, PUT still works.
  const ok = await fetch(`${base}/license%3AENTH-OK`, {
    method: "PUT",
    headers: auth(),
    body: "x".repeat(64),
  });
  assert.equal(ok.status, 200);
});

test("bootstrap exits 1 when GATEWAY_TOKEN is shorter than 32 chars", () => {
  const serverPath = fileURLToPath(new URL("../server.mjs", import.meta.url));
  const r = spawnSync(process.execPath, [serverPath], {
    env: { ...process.env, GATEWAY_TOKEN: "too-short", REDISPASSWORD: "x" },
    encoding: "utf8",
  });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /GATEWAY_TOKEN/);
});
