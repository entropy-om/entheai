# entropy.om GitHub Organization Profile Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generate, publish, and verify the approved public GitHub organization identity for `entropy.om`.

**Architecture:** Generate one square raster avatar from the approved prompt, preserve the selected asset in the repository, and apply the avatar and text through the authenticated GitHub organization settings UI. Verify the deployed result independently through GitHub's public organization API and public organization page.

**Tech Stack:** OpenAI built-in image generation, local image inspection, GitHub organization settings, GitHub REST API via `gh`

## Global Constraints

- Organization login: `entropy-om`.
- Organization name: `entropy.om`.
- Description: `Building sovereign intelligence in the open — local-first agents, memory, orchestration, and tools for minds that fan out.`
- Temporary website: `https://github.com/peterlodri-sec/entheai` until `https://entropy.om` has a DNS record and a live public site.
- Location, public email, and social accounts remain unset.
- Avatar contains no letters, words, watermark, third-party mark, mockup, or 3D presentation.
- Existing unrelated workspace changes remain untouched.

---

### Task 1: Generate and validate the organization avatar

**Files:**
- Create: `docs/images/entropy-om-avatar.png`
- Reference: `docs/superpowers/specs/2026-07-19-entropy-om-github-organization-profile-design.md`

**Interfaces:**
- Consumes: the exact image-generation prompt in the approved design spec
- Produces: a square PNG suitable for GitHub's circular avatar crop at `docs/images/entropy-om-avatar.png`

- [ ] **Step 1: Generate the avatar**

Use OpenAI built-in image generation with the exact prompt from the design spec. Generate a square image with the central amber seed and cyan broken-orbit arcs entirely inside a circular safe area.

- [ ] **Step 2: Preserve the generated asset**

Copy the selected generated PNG to:

```text
docs/images/entropy-om-avatar.png
```

Do not overwrite any existing image with a different name.

- [ ] **Step 3: Validate image dimensions and format**

Run:

```bash
sips -g pixelWidth -g pixelHeight -g format docs/images/entropy-om-avatar.png
```

Expected: equal width and height, PNG format, and dimensions large enough for a GitHub organization avatar.

- [ ] **Step 4: Inspect full-size and small-size appearance**

Inspect `docs/images/entropy-om-avatar.png` visually. Create a non-committed 32-pixel preview with:

```bash
preview_dir=$(mktemp -d)
sips -z 32 32 docs/images/entropy-om-avatar.png --out "$preview_dir/avatar-32.png"
```

Expected: the amber center and broken cyan arcs remain recognizable, the circular crop is safe, and there is no text, watermark, or third-party mark.

- [ ] **Step 5: Commit the approved avatar asset**

```bash
git add docs/images/entropy-om-avatar.png
git commit -m "docs(brand): add entropy.om organization avatar"
```

Expected: the commit includes only `docs/images/entropy-om-avatar.png`.

### Task 2: Deploy the approved GitHub organization profile

**Files:**
- Read: `docs/images/entropy-om-avatar.png`

**Interfaces:**
- Consumes: the validated PNG from Task 1 and the exact public profile fields from Global Constraints
- Produces: the updated `https://github.com/entropy-om` organization profile

- [ ] **Step 1: Open the organization profile settings**

Open:

```text
https://github.com/organizations/entropy-om/settings/profile
```

Use the authenticated browser session. Do not change billing, membership, repository permissions, security controls, or any setting outside the profile page.

- [ ] **Step 2: Upload the avatar**

Select `docs/images/entropy-om-avatar.png` as the organization profile picture. Confirm the crop keeps the complete broken-orbit mark visible before saving.

- [ ] **Step 3: Apply the public profile fields**

Set these values exactly:

```text
Name: entropy.om
Description: Building sovereign intelligence in the open — local-first agents, memory, orchestration, and tools for minds that fan out.
URL: https://github.com/peterlodri-sec/entheai
```

Leave location, public email, and social account fields empty. Save the profile.

- [ ] **Step 4: Verify the REST representation**

Run:

```bash
gh api -H 'Accept: application/vnd.github+json' /orgs/entropy-om \
  --jq '{login, name, description, blog, location, email, twitter_username, avatar_url, updated_at}'
```

Expected: `name`, `description`, and `blog` exactly match the approved values; `location`, `email`, and `twitter_username` are null or empty; `avatar_url` is present; `updated_at` reflects the deployment.

- [ ] **Step 5: Verify the public organization page**

Open `https://github.com/entropy-om` and confirm the avatar, organization name, full description, and temporary website render publicly. If any field differs, correct only that field on the profile settings page and repeat Steps 4 and 5.
