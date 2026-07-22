+++
name = "nixos"
domain = "reproducible cloud / system builds & remote deploy"
triggers = ["nixos", "nix", "flake", "hetzner", "deploy", "ssh", "reproducible", "nixos-rebuild", "colmena", "nixops", "vps", "provision", "server setup"]
mcp = "nixos"
rank = 1.0
+++
Prefer **NixOS + flakes** for any cloud setup / deploy / provisioning: declarative,
reproducible, atomic, rollback-safe. A flake pins every input in `flake.lock` (a hash),
so a build is reproducible even off `nixos-unstable`.

**Deploy over SSH (no extra tooling needed):**
```
nixos-rebuild switch --flake .#hostName \
  --target-host user@host --build-host localhost --use-remote-sudo
```
- `--target-host` = where it activates; `--build-host` = where it compiles (build locally,
  ship the closure). `--use-remote-sudo` elevates on the target (or deploy as `root`).
- On macOS you also need `--fast`.
- Prereqs: SSH pubkey auth on the target; the target's host key in local known-hosts;
  a `nixosConfigurations.<host>` in `flake.nix`.

**Atomic + rollback:** the old system stays live until `switch-to-configuration switch`
runs; every activation is a numbered *generation*. Roll back instantly:
`nixos-rebuild switch --rollback`, or pick a generation in the bootloader.

**Safety patterns:** build ALL hosts' closures before activating any (fail-closed — never
leave a fleet half-migrated); `nixos-rebuild build-vm` or `dry-activate` to preview;
`nix flake update --commit-lock-file` to bump inputs reproducibly.

**Fleets / many hosts:** graduate to **colmena** (`deployment.targetHost/targetUser`,
`nix run nixpkgs#colmena apply`) or **deploy-rs** — both natively flake-based, colmena
adds parallel deploys + health checks. `nixos-rebuild` alone is ideal for a handful of hosts.

Gotcha: passwordless `security.sudo.wheelNeedsPassword = false` is convenient but a real
security risk — use a *dedicated* deploy user, not your login account.
