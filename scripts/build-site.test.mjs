import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { build } from "./build-site.mjs";

function makeFixtureRoot() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "site-fixture-"));
  const contentDir = path.join(root, "public", "docs", "content");
  fs.mkdirSync(contentDir, { recursive: true });
  fs.writeFileSync(
    path.join(root, "public", "docs", "_template.html"),
    "<html><body><nav><!--NAV--></nav><main><!--PAGES--></main><!--MD_SCRIPTS--></body></html>",
    "utf8"
  );
  fs.writeFileSync(
    path.join(contentDir, "what-is.md"),
    "---\nid: what-is\ntitle: What is entheai\ngroup: Overview\norder: 1\nbadgeText: Overview\n---\n\nIt's an agent.\n",
    "utf8"
  );
  fs.writeFileSync(
    path.join(contentDir, "who.md"),
    "---\nid: who\ntitle: Who it's for\ngroup: Overview\norder: 2\n---\n\nSolo developers.\n",
    "utf8"
  );
  return root;
}

test("build() writes docs/index.html, llms.txt, and llms-full.txt", () => {
  const root = makeFixtureRoot();
  build({ root, baseUrl: "https://entheai.com", siteTitle: "entheai", siteSummary: "s" });

  const indexHtml = fs.readFileSync(path.join(root, "public", "docs", "index.html"), "utf8");
  assert.match(indexHtml, /data-page="what-is"/);
  assert.match(indexHtml, /data-page="who" hidden/);
  assert.match(indexHtml, /data-nav-item="what-is"/);
  assert.match(indexHtml, /<script type="text\/markdown" id="md-what-is">/);

  const llmsTxt = fs.readFileSync(path.join(root, "public", "llms.txt"), "utf8");
  assert.match(llmsTxt, /# entheai/);
  assert.match(llmsTxt, /what-is/);

  const llmsFullTxt = fs.readFileSync(path.join(root, "public", "llms-full.txt"), "utf8");
  assert.match(llmsFullTxt, /It's an agent\./);
});

test("build() throws a clear error if the template is missing a marker", () => {
  const root = makeFixtureRoot();
  fs.writeFileSync(
    path.join(root, "public", "docs", "_template.html"),
    "<html><body>no markers here</body></html>",
    "utf8"
  );
  assert.throws(
    () => build({ root, baseUrl: "https://entheai.com", siteTitle: "entheai", siteSummary: "s" }),
    /missing <!--NAV--> marker/
  );
});
