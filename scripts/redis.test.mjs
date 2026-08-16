// scripts/redis.test.mjs — redisHttp() (the gateway HTTP path) contract tests.
// Runs as part of `npm test` (glob: scripts/**/*.test.mjs). No network:
// globalThis.fetch is stubbed per test.

import test from "node:test";
import assert from "node:assert/strict";
import { redis, redisHttp } from "../src/redis.mjs";

function jsonResponse(status, body) {
  return new Response(JSON.stringify(body), { status });
}

test("redisHttp GET returns the value for a hit, null on 404 (missing key)", async () => {
  const realFetch = globalThis.fetch;
  const calls = [];
  globalThis.fetch = async (url, init) => {
    calls.push({ url: String(url), init });
    if (/ENTH-HIT/.test(String(url))) return jsonResponse(200, { value: "license-json" });
    return jsonResponse(404, { error: "not found" });
  };
  try {
    assert.equal(
      await redisHttp("https://gw.up.railway.app", "tok", "GET", "license:ENTH-HIT"),
      "license-json"
    );
    assert.equal(await redisHttp("https://gw.up.railway.app", "tok", "GET", "license:ENTH-MISS"), null);
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(calls.length, 2);
  assert.equal(calls[0].url, "https://gw.up.railway.app/license%3AENTH-HIT");
  assert.equal(calls[0].init.method, "GET");
  assert.equal(calls[0].init.headers.Authorization, "Bearer tok");
  assert.equal(calls[0].init.body, undefined);
});

test("redisHttp GET tolerates a missing { value } and non-404 failures throw", async () => {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    if (/NOVALUE/.test(String(url))) return jsonResponse(200, { ok: true }); // no value field
    if (/DOWN/.test(String(url))) return jsonResponse(503, { error: "redis unreachable" });
    return new Response("not-json", { status: 200 }); // invalid JSON
  };
  try {
    assert.equal(await redisHttp("https://gw", "tok", "GET", "key:NOVALUE"), null);
    await assert.rejects(
      redisHttp("https://gw", "tok", "GET", "key:BADJSON"),
      /non-JSON/
    );
    await assert.rejects(
      redisHttp("https://gw", "tok", "GET", "key:DOWN"),
      /-> 503/
    );
  } finally {
    globalThis.fetch = realFetch;
  }
});

test("redisHttp requires both the base URL and the token", async () => {
  await assert.rejects(
    redisHttp("", "tok", "GET", "k"),
    /REDIS_GATEWAY_URL/
  );
  await assert.rejects(
    redisHttp("https://gw", "", "GET", "k"),
    /REDIS_GATEWAY_TOKEN/
  );
});

test("redis() refuses a plaintext redis:// URL that carries a password", async () => {
  // Throws before the cloudflare:sockets import, so this is testable in Node.
  await assert.rejects(
    redis(new URL("redis://default:pw@host.example:6379"), "GET", "k"),
    /plaintext redis:\/\/ with a password/
  );
  // rediss:// with a password passes the guard — its failure (sockets only
  // exist in workerd) must NOT be the plaintext refusal.
  await assert.rejects(
    redis(new URL("rediss://default:pw@host.example:6379"), "GET", "k"),
    (err) => !/plaintext redis/.test(String(err && err.message))
  );
});
