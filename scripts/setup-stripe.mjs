#!/usr/bin/env node
// One-shot Stripe setup for the entheai Backer tier.
//
// Creates a one-time Product + Price (EUR 9.99) and a hosted Payment Link,
// then wires the resulting `https://buy.stripe.com/...` URL into
// `public/back.html` (replacing the placeholder). Run once per mode:
//
//   STRIPE_SECRET_KEY=sk_test_... node scripts/setup-stripe.mjs   # test mode
//   STRIPE_SECRET_KEY=sk_live_... node scripts/setup-stripe.mjs   # live
//
// Uses the raw REST API over fetch (no stripe-node) so it runs anywhere.
// The Payment Link redirects buyers to the claim page with the checkout
// session id, which the Worker uses to look up the license key it issued.

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, "..");

const PRODUCT_NAME = "entheai Backer";
const CURRENCY = "eur";
const UNIT_AMOUNT = 999; // €9.99 in minor units
const CLAIM_URL = "https://entheai.com/back/claim?session_id={CHECKOUT_SESSION_ID}";

const key = process.env.STRIPE_SECRET_KEY;
if (!key) {
  console.error("error: STRIPE_SECRET_KEY is not set");
  process.exit(1);
}
const mode = key.startsWith("sk_live_") || key.startsWith("rk_live_")
  ? "live"
  : "test";
console.log(`[setup-stripe] mode=${mode} product="${PRODUCT_NAME}" amount=${UNIT_AMOUNT} ${CURRENCY}`);

async function stripe(method, path, params) {
  const body = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    body.append(k, v);
  }
  const res = await fetch(`https://api.stripe.com/v1${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${key}`,
      "Content-Type": "application/x-www-form-urlencoded",
    },
    body,
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(`Stripe ${method} ${path} -> ${res.status}: ${json.error?.message ?? JSON.stringify(json)}`);
  }
  return json;
}

async function main() {
  // Create the one-time Price inline (Stripe creates the Product for us).
  const link = await stripe("POST", "/payment_links", {
    "line_items[0][price_data][currency]": CURRENCY,
    "line_items[0][price_data][product_data][name]": PRODUCT_NAME,
    "line_items[0][price_data][unit_amount]": String(UNIT_AMOUNT),
    "line_items[0][quantity]": "1",
    // This account has Managed Payments on by default, which requires a
    // product tax_code for inline price_data. We're selling a simple digital
    // key, so disable Managed Payments on this link rather than classify it.
    "managed_payments[enabled]": "false",
    "after_completion[type]": "redirect",
    "after_completion[redirect][url]": CLAIM_URL,
  });

  const buyUrl = link.url;
  if (!buyUrl) {
    throw new Error("Stripe returned no Payment Link URL");
  }
  console.log(`[setup-stripe] payment link created: ${buyUrl}`);

  // Wire the URL into public/back.html.
  const backHtml = resolve(REPO_ROOT, "public", "back.html");
  const html = readFileSync(backHtml, "utf8");
  const placeholder = /https:\/\/buy\.stripe\.com\/REPLACE_WITH_YOUR_PAYMENT_LINK/;
  if (!placeholder.test(html)) {
    console.warn(
      "[setup-stripe] warning: placeholder not found in public/back.html — " +
        "paste the URL manually into the Buy now <a href>.",
    );
  } else {
    const updated = html.replace(placeholder, buyUrl).replace(
      /<!-- TODO: paste real Stripe Payment Link URL here -->\n/,
      "",
    );
    writeFileSync(backHtml, updated);
    console.log(`[setup-stripe] wired URL into public/back.html`);
  }

  console.log("\nRemaining manual steps (one-time, not in this script):");
  console.log("  1. Webhook: Dashboard -> Developers -> Webhooks -> Add endpoint");
  console.log("     URL: https://entheai.com/api/stripe/webhook");
  console.log("     Events: checkout.session.completed");
  console.log("     Then: wrangler secret put STRIPE_WEBHOOK_SECRET   (paste the whsec_...)");
  console.log("  2. KV:     wrangler kv namespace create LICENSES");
  console.log("     -> paste the id over REPLACE_WITH_LICENSES_KV_NAMESPACE_ID in wrangler.jsonc");
  console.log("  3. Deploy: npm run build && npx wrangler deploy");
  console.log("  4. Test:   open the link and pay with card 4242 4242 4242 4242");
}

main().catch((e) => {
  console.error(`[setup-stripe] ${e.message}`);
  process.exit(1);
});
