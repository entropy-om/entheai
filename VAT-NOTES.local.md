# VAT & invoicing — the plain-language version

**TL;DR: you're basically done.** You already have the three hard things —
a *verified Stripe account*, an *accountant*, and an *EU VAT number*. What's
left is about **10 minutes of clicking in the Stripe Dashboard, once.** The
scary-sounding tax math is now automatic.

---

## The one confusing thing, cleared up

Your **EU VAT number (HU69757200)** is your *real-world* registration — it's how
the Hungarian tax office (NAV) knows you. That part is 100% done and real.

Stripe is a **separate system**. It does *not* automatically know you're
VAT-registered just because you own a number. It needs its own little switch —
a "**tax registration**" record — that says *"collect 27% Hungarian VAT on my
sales."* **I already flipped that switch in your live account** (it's active
now). So from this moment, every payment gets its VAT calculated and recorded
automatically.

The analogy:

- **Your VAT number** = your driver's license (proof you're allowed to drive).
- **Stripe's tax registration** = actually turning the key and starting the car.

You had the license. I started the car.

---

## What happens now, per customer

Your prices are **VAT-inclusive** — the customer always pays **exactly €10 or
€20**, never a surprise "+VAT" on top. What changes is how that amount is *split*
internally (which is what your accountant cares about):

| Who buys | Pays | Stripe records (on €10) | Who deals with the VAT |
|---|---|---|---|
| 🇭🇺 Hungarian person | €10 | €7.87 you + €2.13 VAT (27%) | You remit €2.13 — your accountant, in your normal ÁFA return |
| 🇪🇺 EU person (other country) | €10 | same split, HU VAT (for now) | same — while you're small (see threshold) |
| 🇪🇺 EU **business** with a VAT ID | €10 | €10 to you, €0 VAT | *they* self-account ("reverse charge") — Stripe does this automatically when they type their VAT ID at checkout |
| 🌍 Non-EU | €10 | usually €0 VAT | nothing |

(The €20 tier splits the same way: €15.75 you + €4.25 VAT.)

You never do this per-customer. Stripe calculates all of it. Your accountant just
reads the totals once a period.

---

## What you still need to do — ~10 min, one time

1. **Put your VAT number on your invoices.**
   Dashboard → **Settings → Business** (and **Tax → Manage settings**) → add
   **HU69757200** as your VAT ID so it prints on every invoice. *(This is the
   "show the number on invoices" item — required for a proper EU invoice, and
   it's the one thing I couldn't set for you via the API.)*

2. **Turn on invoice/receipt emails** (if not already):
   Dashboard → **Settings → Customer emails** → enable *Successful payments* and
   *Invoices*, so customers get their receipt automatically.

3. **Show your accountant where the numbers are.**
   Dashboard → **Tax** → the registration/reports view shows exactly how much VAT
   you collected each period. Hand that to your accountant; they fold it into your
   normal Hungarian VAT return. This is routine for them.

That's the whole list.

---

## What you do NOT need (stop worrying about these)

- ❌ **OSS registration** (the EU "one-stop-shop") — only matters once your
  **cross-border sales to EU *consumers* pass €10,000/year.** Below that, charging
  Hungarian VAT on everything is correct *and* simplest. You're nowhere near it.
  When you get there: one more Stripe switch + your accountant enrolls you with
  NAV. Future-you's problem, not today's.
- ❌ Registering for tax in other countries — no.
- ❌ Anything special for business customers — reverse charge is automatic.
- ❌ Calculating any VAT by hand — Stripe does it.

---

## If you remember one thing

You already had the hard parts. I turned on Stripe's VAT collection and verified
it works. Your only homework: **add your VAT number in the Dashboard so it shows
on invoices**, and **show your accountant where the Stripe tax report lives.**
Everything else runs itself.

---

*This file is **local only** — not committed, not published. It's yours.
Location: `entheai/VAT-NOTES.local.md`. — as things are, nothing more, nothing less.*
