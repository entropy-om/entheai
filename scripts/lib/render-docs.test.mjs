import test from "node:test";
import assert from "node:assert/strict";
import {
  transformCallouts,
  styleCodeBlocks,
  styleTables,
  styleInlineCode,
  addHeadingIds,
  renderPageBody,
  renderPageHtml,
  renderNavHtml,
} from "./render-docs.mjs";

test("transformCallouts converts a NOTE blockquote to callout markup", () => {
  const md = "> [!NOTE]\n> This is a note.\n\nAfter.";
  const out = transformCallouts(md);
  assert.match(out, /<div class="callout callout-note">/);
  assert.match(out, /<div class="callout-label">Note<\/div>/);
  assert.match(out, /<div class="callout-body">This is a note\.<\/div>/);
  assert.match(out, /After\.$/);
});

test("transformCallouts renders inline code inside the callout body with the ic class", () => {
  const md = "> [!TIP]\n> Use `--yolo` carefully.";
  const out = transformCallouts(md);
  assert.match(out, /<code class="ic">--yolo<\/code>/);
});

test("styleCodeBlocks wraps a fenced code block in the codeblock chrome", () => {
  const html = '<pre><code class="language-bash">echo hi\n</code></pre>';
  const out = styleCodeBlocks(html);
  assert.match(out, /<div class="codeblock">/);
  assert.match(out, /<span>bash<\/span>/);
  assert.match(out, /data-copy="echo hi\n"/);
  assert.match(out, /<pre><code>echo hi\n<\/code><\/pre>/);
});

test("styleTables adds key-table class and per-column cell classes", () => {
  const html =
    "<table><thead><tr><th>Key</th><th>Type</th><th>Description</th></tr></thead>" +
    "<tbody><tr><td>router.plan</td><td>model id</td><td>Plans.</td></tr></tbody></table>";
  const out = styleTables(html);
  assert.match(out, /<table class="key-table">/);
  assert.match(out, /<td class="key">router\.plan<\/td>/);
  assert.match(out, /<td class="type">model id<\/td>/);
  assert.match(out, /<td class="desc">Plans\.<\/td>/);
});

test("styleInlineCode adds the ic class to inline code but not code inside pre blocks", () => {
  const html = "<p>Use <code>PATH</code>.</p><pre><code>raw text</code></pre>";
  const out = styleInlineCode(html);
  assert.match(out, /<code class="ic">PATH<\/code>/);
  assert.match(out, /<pre><code>raw text<\/code><\/pre>/);
});

test("addHeadingIds prefixes h2 ids with the page id", () => {
  const html = "<h2>Core Keys</h2>";
  const out = addHeadingIds(html, "configure");
  assert.match(out, /<h2 id="configure-core-keys">Core Keys<\/h2>/);
});

test("renderPageBody produces callouts, tables, code blocks, and heading ids together", () => {
  const page = {
    id: "configure",
    body:
      "Intro text.\n\n" +
      "```bash\necho hi\n```\n\n" +
      "## Core keys\n\n" +
      "| Key | Type | Description |\n|---|---|---|\n| router.plan | model id | Plans. |\n\n" +
      "> [!TIP]\n> Use `--yolo`.",
  };
  const html = renderPageBody(page);
  assert.match(html, /<div class="codeblock">/);
  assert.match(html, /<table class="key-table">/);
  assert.match(html, /<h2 id="configure-core-keys">/);
  assert.match(html, /<div class="callout callout-tip">/);
});

test("renderPageHtml includes a badge when badgeText is set, escapes title, and hides non-first pages", () => {
  const page = {
    id: "models",
    title: "Models & ids",
    badgeText: null,
    badgeColor: "teal",
    body: "Body text.",
  };
  const first = renderPageHtml(page, true);
  const notFirst = renderPageHtml(page, false);
  assert.match(first, /<div data-page="models">/);
  assert.match(notFirst, /<div data-page="models" hidden>/);
  assert.match(first, /<h1>Models &amp; ids<\/h1>/);
  assert.doesNotMatch(first, /class="badge/);

  const badged = renderPageHtml(
    { id: "what-is", title: "What is entheai", badgeText: "Overview", badgeColor: "teal", body: "x" },
    true
  );
  assert.match(badged, /<span class="badge badge-teal"[^>]*>Overview<\/span>/);
});

test("renderPageHtml includes a Copy page button wired to the page id", () => {
  const page = { id: "who", title: "Who", badgeText: null, badgeColor: "teal", body: "x" };
  const html = renderPageHtml(page, true);
  assert.match(html, /<button class="copy-btn copy-page-btn" data-copy-page="who">Copy page<\/button>/);
});

test("renderNavHtml groups pages and escapes ampersands", () => {
  const pages = [
    { id: "models", title: "Models", navTitle: "Models & ids", group: "Configuration", order: 2 },
    { id: "providers", title: "Providers", navTitle: "Providers", group: "Configuration", order: 1 },
  ];
  const html = renderNavHtml(pages, ["Configuration"]);
  assert.match(html, /<div class="nav-group" data-nav-group="Configuration">/);
  const providersIdx = html.indexOf("providers");
  const modelsIdx = html.indexOf("models");
  assert.ok(providersIdx < modelsIdx, "providers (order 1) should come before models (order 2)");
  assert.match(html, /Models &amp; ids/);
});
