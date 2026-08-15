// gateway/test/gateway.test.mjs — unit tests for the gateway handler against a
// fake in-memory store, plus a real-HTTP smoke test through startServer().
// Run: node --test test/   (or npm test from gateway/)

import test from "node:test";
import assert from "node:assert/strict";
import { createHandler } from "../handler.mjs";
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
