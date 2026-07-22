# Human task — set up macOS code-signing + notarization for entheai releases

**Why this is a human task:** signing needs your **Developer ID Application** certificate's
*private key*, and notarization needs an **App Store Connect API key** — both require your
Apple ID login and keychain password, which the agent won't touch. Once the six GitHub
secrets at the bottom exist, the agent wires the CI and releases become properly
signed + notarized (no "unidentified developer" Gatekeeper warning).

**What the search found (2026-07-22, on this M3 `HFK99M239G`):**
`security find-identity -v -p codesigning` → **0 valid identities.** No Developer ID cert,
no notary credentials, no provisioning profiles here. Xcode 26.6 + notarytool 1.1.2 present.
→ The cert is on your **M1** (check there first) or needs creating fresh (Step 1).

---

## Step 0 — is it already on the M1?

On the M1, run:
```sh
security find-identity -v -p codesigning
```
If you see `Developer ID Application: <Your Name> (TEAMID)` → skip to **Step 3** (export it).
If not → do Step 1 on whichever Mac you'll export from.

## Step 1 — create the Developer ID Application certificate (Xcode, easiest)

You have the paid Developer Program, so:

1. **Xcode → Settings → Accounts** → select your Apple ID → **Manage Certificates…**
2. Click **+** (bottom-left) → **Developer ID Application**.
3. It generates the cert + private key straight into your **login keychain**.

*(Alternative, no Xcode — developer.apple.com → Certificates, Identifiers & Profiles →
Certificates → **+** → "Developer ID Application" → upload a CSR made in Keychain Access →
Certificate Assistant → "Request a Certificate from a Certificate Authority" (saved to disk),
then download the `.cer` and double-click to install.)*

## Step 2 — confirm the identity + Team ID

```sh
security find-identity -v -p codesigning
# → 1) ABCD…  "Developer ID Application: Your Name (TEAMID)"
```
Note the **full quoted string** (that's `MACOS_SIGN_IDENTITY`) and the **TEAMID** in parens
(also visible at developer.apple.com → Membership details → Team ID).

## Step 3 — export the cert + private key as a base64 secret

In **Keychain Access** → **login** keychain → **My Certificates**:
1. Find **Developer ID Application: Your Name (TEAMID)** — expand it so the little key shows
   underneath (you must export the cert *and* its private key together).
2. Right-click it → **Export "Developer ID Application…"** → save as `DeveloperID.p12`.
3. Set an **export password** — remember it (this becomes `MACOS_CERTIFICATE_PWD`).

Then base64-encode it for GitHub:
```sh
base64 -i DeveloperID.p12 | pbcopy   # copies the blob → paste into the MACOS_CERTIFICATE secret
```
Delete `DeveloperID.p12` afterwards — it's your private key.

## Step 4 — App Store Connect API key (for notarization)

The modern, CI-friendly way (no app-specific password juggling):
1. **appstoreconnect.apple.com → Users and Access → Integrations → App Store Connect API**
   (Team Keys) → **Generate API Key** (or **+**).
2. Name it `entheai-ci`, role **Developer** → **Generate**.
3. **Download the `.p8` (you can only download it ONCE)**. Note the **Key ID** and, at the top
   of the Keys page, the **Issuer ID** (a UUID).
4. Base64 it:
   ```sh
   base64 -i AuthKey_XXXXXXXX.p8 | pbcopy   # → NOTARY_KEY secret
   ```

## Step 5 — add the GitHub secrets

Repo → **Settings → Secrets and variables → Actions → New repository secret** (or via CLI on a
Mac where you're `gh auth`'d):

| Secret | Value |
|--------|-------|
| `MACOS_CERTIFICATE` | base64 of `DeveloperID.p12` (Step 3) |
| `MACOS_CERTIFICATE_PWD` | the `.p12` export password (Step 3) |
| `MACOS_SIGN_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` (Step 2) |
| `KEYCHAIN_PWD` | any strong throwaway string (CI's temp keychain password) |
| `NOTARY_KEY` | base64 of the `.p8` API key (Step 4) |
| `NOTARY_KEY_ID` | the API Key ID (Step 4) |
| `NOTARY_ISSUER_ID` | the API Issuer ID UUID (Step 4) |

CLI form (run on the M1/M3 where `gh` is logged in):
```sh
gh secret set MACOS_CERTIFICATE     --repo entropy-om/entheai < <(base64 -i DeveloperID.p12)
gh secret set MACOS_CERTIFICATE_PWD --repo entropy-om/entheai   # paste when prompted
gh secret set MACOS_SIGN_IDENTITY   --repo entropy-om/entheai   # "Developer ID Application: … (TEAMID)"
gh secret set KEYCHAIN_PWD          --repo entropy-om/entheai   # any strong string
gh secret set NOTARY_KEY            --repo entropy-om/entheai < <(base64 -i AuthKey_XXXX.p8)
gh secret set NOTARY_KEY_ID         --repo entropy-om/entheai   # the Key ID
gh secret set NOTARY_ISSUER_ID      --repo entropy-om/entheai   # the Issuer UUID
```

## Step 6 — tell the agent "secrets are set"

Then it wires `release.yml` with the job below (pre-written, tested shape — it imports the
cert into a temporary keychain, signs with hardened runtime, notarizes via notarytool, staples,
and packages a signed tarball). Nothing here needs your private key again.

```yaml
      - name: Import Developer ID cert into a temp keychain
        env:
          MACOS_CERTIFICATE: ${{ secrets.MACOS_CERTIFICATE }}
          MACOS_CERTIFICATE_PWD: ${{ secrets.MACOS_CERTIFICATE_PWD }}
          KEYCHAIN_PWD: ${{ secrets.KEYCHAIN_PWD }}
        run: |
          echo "$MACOS_CERTIFICATE" | base64 -d > cert.p12
          security create-keychain -p "$KEYCHAIN_PWD" build.keychain
          security default-keychain -s build.keychain
          security unlock-keychain -p "$KEYCHAIN_PWD" build.keychain
          security import cert.p12 -k build.keychain -P "$MACOS_CERTIFICATE_PWD" -T /usr/bin/codesign
          security set-key-partition-list -S apple-tool:,apple: -s -k "$KEYCHAIN_PWD" build.keychain
          rm cert.p12

      - name: Sign (hardened runtime)
        env:
          IDENTITY: ${{ secrets.MACOS_SIGN_IDENTITY }}
        run: |
          codesign --force --options runtime --timestamp \
            --sign "$IDENTITY" \
            target/aarch64-apple-darwin/release/entheai
          codesign --verify --strict --verbose=2 target/aarch64-apple-darwin/release/entheai

      - name: Notarize + staple
        env:
          NOTARY_KEY: ${{ secrets.NOTARY_KEY }}
          NOTARY_KEY_ID: ${{ secrets.NOTARY_KEY_ID }}
          NOTARY_ISSUER_ID: ${{ secrets.NOTARY_ISSUER_ID }}
        run: |
          echo "$NOTARY_KEY" | base64 -d > AuthKey.p8
          # ditto a zip for submission (notarytool wants a zip/pkg/dmg, not a bare binary)
          ditto -c -k --keepParent target/aarch64-apple-darwin/release/entheai entheai-notarize.zip
          xcrun notarytool submit entheai-notarize.zip \
            --key AuthKey.p8 --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" --wait
          rm AuthKey.p8
          # bare CLI binaries can't be stapled; ship inside a .app/.dmg/.pkg to staple.
          # Since the launcher builds entheai.app, staple that instead for the app bundle path.
```

> Note: a bare Mach-O CLI binary is **notarized but not staple-able** (stapling needs an
> `.app`/`.dmg`/`.pkg`). For a fully offline-verifiable download, we should ship the
> **signed `.app`/`.dmg`** (your launcher already builds `entheai.app`) — the agent will
> adjust Step 6 to sign+notarize+staple that bundle when we wire it. The Homebrew tarball path
> can stay as-is (Homebrew handles quarantine) but will now carry a real Developer ID signature.

---

### After this
Ping the agent: *"apple signing secrets are set"* → it edits `release.yml` (the job above,
adapted to sign+notarize+staple the `.app`/`.dmg`), and the next `v*` tag cuts a proper,
notarized macOS release. Membership: **Apple Developer Program** (you have it) — no extra
purchase needed.
