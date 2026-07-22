+++
name = "docker"
domain = "containers / portable runtime packaging"
triggers = ["docker", "dockerfile", "container", "image", "oci", "ghcr", "docker compose", "distroless"]
mcp = "docker"
rank = 0.9
+++
When something must run **the same everywhere** and NixOS isn't the target, containerize it.
(For reproducible *systems*, NixOS is stronger; Docker wins for portable app packaging + a
huge ecosystem.)

**Image best-practice:**
- **Multi-stage build:** a fat builder stage → copy only the artifact into a tiny runtime
  (`debian:slim`, or **distroless** / `scratch` for static binaries). Ship the binary, not
  the toolchain.
- **Non-root:** create + `USER` an unprivileged user; drop caps.
- **Layer cache:** copy dependency manifests + fetch deps *before* copying source, so a code
  change doesn't bust the dep layer. Pin the base by digest for reproducibility.
- **.dockerignore** aggressively (`.git`, `target/`, `node_modules/`) — smaller context,
  faster builds.
- One process per container; config + secrets via env/mounts, never baked into the image.

**Publish:** GHCR (`ghcr.io/<org>/<img>`) via `docker/build-push-action` with
`docker/metadata-action` tags + gha layer cache; build on tags, not every push.
