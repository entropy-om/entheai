import test from "node:test";
import assert from "node:assert/strict";
import {
  handleEntropy,
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
