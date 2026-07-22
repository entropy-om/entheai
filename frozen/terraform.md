+++
name = "terraform"
domain = "declarative cloud infrastructure provisioning"
triggers = ["terraform", "opentofu", "iac", "infrastructure as code", "provision", "aws", "gcp", "cloudflare", "hcloud", "state file"]
rank = 0.85
+++
For provisioning cloud *resources* (VMs, networks, DNS, buckets) declaratively — the layer
below the OS that NixOS then configures — use **Terraform / OpenTofu**. (NixOS builds the
machine; Terraform creates + wires the machines. They compose: TF makes the Hetzner box,
NixOS configures it.)

**Practices:**
- **Remote, locked state:** never local state for shared infra — an S3/GCS/TF-Cloud backend
  with state locking so two applies can't race/corrupt it. State can hold secrets → encrypt +
  restrict.
- **Plan before apply, always:** `terraform plan` reviewed (in CI, as a PR comment) before
  `apply`. Applies gated on human approval for prod (the graph "human-in-the-loop" gate).
- **Modules:** small, reusable, versioned modules; pin provider + module versions; a
  `dev/staging/prod` workspace or dir split, DRY via modules not copy-paste.
- **Immutable > mutable:** replace resources rather than in-place mutate where cheap;
  `prevent_destroy` on the precious ones.
- Prefer **OpenTofu** (open-source fork) if licensing matters; the HCL is compatible.
