# entheai Redis HTTP gateway

A tiny TLS-terminating HTTP gateway in front of the entheai Worker's Railway
Redis store. Zero runtime dependencies — plain `node:http` plus a minimal
RESP2 client over `node:net` (same wire format the Worker's `src/redis.mjs`
speaks).

## Why this exists

The Worker previously reached Railway Redis over public TCP via
`REDIS_PUBLIC_URL`, which meant the Redis password transited Cloudflare →
Railway in plaintext. This gateway closes that gap:

- It runs **on Railway, in the same project** as the Redis database, so it
  reaches Redis over the private `redis.railway.internal` network.
- Railway terminates TLS at its edge (auto-TLS on the public domain), so the
  Worker now only ever sends `GATEWAY_TOKEN` over HTTPS — the Redis password
  never leaves Railway's private network.

```
Cloudflare Worker ──HTTPS──▶ Railway edge (auto-TLS) ──▶ gateway ──private net──▶ Redis
  sends GATEWAY_TOKEN only                    (this service)   REDISPASSWORD stays here
```

## Endpoints

Every request must carry `Authorization: Bearer <GATEWAY_TOKEN>`; anything
else gets `401 {"error":"unauthorized"}`. Keys are percent-encoded
(`encodeURIComponent`), e.g. `license%3AENTH-XXXX` for `license:ENTH-XXXX`.

| Method   | Path       | Body          | Success                          | Errors |
|----------|------------|---------------|----------------------------------|--------|
| `GET`    | `/:key`    | —             | `200 {"value":"..."}`            | `404 {"error":"not found"}` |
| `PUT`    | `/:key`    | raw string    | `200 {"ok":true}`                | —      |
| `DELETE` | `/:key`    | —             | `200 {"ok":true}`                | —      |
| `GET`    | `/health`  | —             | `200 {"ok":true}` (pings Redis)  | `503 {"error":"redis unreachable"}` |

Example keys: `license:ENTH-...`, `session:cs_...`, `vkd:license:...`,
`releases:beta`.

## Env vars

| Var            | Default                    | Required | Notes |
|----------------|----------------------------|----------|-------|
| `REDIS_HOST`   | `redis.railway.internal`   | —        | Private-network hostname; never expose a public host |
| `REDIS_PORT`   | `6379`                     | —        | |
| `REDISUSER`    | `default`                  | —        | Redis ACL user |
| `REDISPASSWORD`| —                          | **yes**  | Still required on the private network (AUTH) |
| `GATEWAY_TOKEN`| —                          | **yes**  | Bearer token the Worker sends; fails fast if unset |
| `PORT`         | `8080`                     | —        | Railway injects `PORT` |

## Run locally

```bash
cd gateway
npm install              # writes package-lock.json (no deps)
GATEWAY_TOKEN=dev-token \
REDISPASSWORD=... \
node server.mjs          # or: npm start
```

## Deploy to Railway

1. From the Railway dashboard, open the project that owns the Redis database.
2. **New Service → Deploy from Dockerfile** pointing at `gateway/` (or link
   this repo and set the Dockerfile path to `gateway/Dockerfile`).
3. Attach the Redis database (variable reference) or set the vars above:
   `REDIS_HOST` / `REDIS_PORT` / `REDISUSER` / `REDISPASSWORD` —
   prefer `${{ Redis.REDIS_PASSWORD }}`-style references so the password is
   never hardcoded. Generate a strong `GATEWAY_TOKEN` (e.g.
   `openssl rand -hex 32`).
4. Railway assigns a public domain (auto-TLS). Set the Worker's secrets:
   - `REDIS_GATEWAY_URL=https://<your-gateway>.up.railway.app`
   - `REDIS_GATEWAY_TOKEN=<the token>`
   and remove `REDIS_PUBLIC_URL` (or leave it as a fallback — the Worker
   prefers the gateway whenever `REDIS_GATEWAY_URL` + `REDIS_GATEWAY_TOKEN`
   are both set).

## Security posture

- **HTTPS terminates at Railway.** The public link only carries
  `GATEWAY_TOKEN` — an app-level secret, not the Redis password.
- **The Redis password never leaves Railway's private network.** The gateway
  connects to `redis.railway.internal` over the private link; the password
  exists only in Railway's env and in the Redis AUTH handshake.
- The gateway binds `0.0.0.0` and relies on Railway's TLS edge + the bearer
  token. Token comparison is constant-time (`crypto.timingSafeEqual`).

## Tests

```bash
cd gateway
npm test                 # node --test test/ — auth + routes against a fake store
```
