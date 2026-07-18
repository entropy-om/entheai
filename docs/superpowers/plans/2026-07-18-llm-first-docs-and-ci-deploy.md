# LLM-first docs + automated CI deploy — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the hand-authored `public/docs/index.html` into a markdown-driven build (source `.md` files → generated HTML + `llms.txt` + `llms-full.txt` + copy-as-markdown), automate deploys via GitHub Actions on push to `main`, and add a frontend-scoped `AGENTS.md`.

**Architecture:** A small Node build script (`scripts/build-site.mjs`, backed by two library modules: `parse-docs.mjs` for frontmatter/validation and `render-docs.mjs`/`render-llms.mjs` for markdown→HTML/text rendering) reads `public/docs/content/*.md`, substitutes generated nav/page/markdown-script HTML into `public/docs/_template.html`, and writes `public/docs/index.html`, `public/llms.txt`, `public/llms-full.txt` as gitignored build artifacts. GitHub Actions runs this build then `wrangler deploy` on every push to `main` that touches site-relevant paths.

**Tech Stack:** Node.js (built-in `node:test` for tests, ESM `.mjs`), `marked` (markdown→HTML), `gray-matter` (frontmatter parsing), `wrangler` + `cloudflare/wrangler-action@v3` (deploy), GitHub Actions.

Spec: `docs/superpowers/specs/2026-07-18-llm-first-docs-and-ci-deploy-design.md`

---

### Task 1: Add build dependencies

**Files:**
- Modify: `package.json`

- [ ] **Step 1: Install `marked` and `gray-matter` as dev dependencies**

Run:
```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
unset -f node npm npx nvm 2>/dev/null
export PATH="$HOME/.nvm/versions/node/v24.17.0/bin:$PATH"
npm install -D marked@^15 gray-matter@^4
```
Expected: `package.json` `devDependencies` gains `marked` and `gray-matter`; `package-lock.json` updates.

- [ ] **Step 2: Add `build` and `test` scripts to package.json**

Edit `package.json` `"scripts"` block to:
```json
"scripts": {
  "build": "node scripts/build-site.mjs",
  "test": "node --test scripts"
}
```

- [ ] **Step 3: Commit**

```bash
git add package.json package-lock.json
git commit -m "build: add marked + gray-matter for the docs build pipeline"
```

---

### Task 2: `parse-docs.mjs` — frontmatter parsing, validation, sorting

**Files:**
- Create: `scripts/lib/parse-docs.mjs`
- Create: `scripts/lib/parse-docs.test.mjs`

- [ ] **Step 1: Write the failing tests**

Create `scripts/lib/parse-docs.test.mjs`:
```javascript
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { loadPages, GROUP_ORDER } from "./parse-docs.mjs";

function makeFixtureDir(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "docs-fixture-"));
  for (const [name, content] of Object.entries(files)) {
    fs.writeFileSync(path.join(dir, name), content, "utf8");
  }
  return dir;
}

test("loads pages sorted by group order then order field", () => {
  const dir = makeFixtureDir({
    "b.md": `---\nid: b\ntitle: B\ngroup: Overview\norder: 2\n---\nBody B\n`,
    "a.md": `---\nid: a\ntitle: A\ngroup: Overview\norder: 1\n---\nBody A\n`,
    "c.md": `---\nid: c\ntitle: C\ngroup: "Getting started"\norder: 1\n---\nBody C\n`,
  });
  const pages = loadPages(dir);
  assert.deepEqual(
    pages.map((p) => p.id),
    ["a", "b", "c"]
  );
  assert.equal(pages[0].body.trim(), "Body A");
});

test("navTitle defaults to title when absent", () => {
  const dir = makeFixtureDir({
    "a.md": `---\nid: a\ntitle: A Title\ngroup: Overview\norder: 1\n---\nBody\n`,
  });
  const pages = loadPages(dir);
  assert.equal(pages[0].navTitle, "A Title");
});

test("navTitle overrides title when present", () => {
  const dir = makeFixtureDir({
    "a.md": `---\nid: a\ntitle: Long Title\nnavTitle: Short\ngroup: Overview\norder: 1\n---\nBody\n`,
  });
  const pages = loadPages(dir);
  assert.equal(pages[0].navTitle, "Short");
  assert.equal(pages[0].title, "Long Title");
});

test("badgeColor defaults to teal when badgeText present", () => {
  const dir = makeFixtureDir({
    "a.md": `---\nid: a\ntitle: A\ngroup: Overview\norder: 1\nbadgeText: Overview\n---\nBody\n`,
  });
  const pages = loadPages(dir);
  assert.equal(pages[0].badgeText, "Overview");
  assert.equal(pages[0].badgeColor, "teal");
});

test("throws on missing required field", () => {
  const dir = makeFixtureDir({
    "a.md": `---\nid: a\ngroup: Overview\norder: 1\n---\nBody\n`,
  });
  assert.throws(() => loadPages(dir), /missing required frontmatter field "title"/);
});

test("throws on unknown group", () => {
  const dir = makeFixtureDir({
    "a.md": `---\nid: a\ntitle: A\ngroup: Nonsense\norder: 1\n---\nBody\n`,
  });
  assert.throws(() => loadPages(dir), /unknown group "Nonsense"/);
});

test("throws on duplicate id", () => {
  const dir = makeFixtureDir({
    "a.md": `---\nid: dup\ntitle: A\ngroup: Overview\norder: 1\n---\nBody\n`,
    "b.md": `---\nid: dup\ntitle: B\ngroup: Overview\norder: 2\n---\nBody\n`,
  });
  assert.throws(() => loadPages(dir), /Duplicate page id "dup"/);
});

test("GROUP_ORDER matches the seven known nav groups", () => {
  assert.deepEqual(GROUP_ORDER, [
    "Overview",
    "Getting started",
    "Configuration",
    "Concepts",
    "The visual TUI",
    "Architecture",
    "Roadmap",
  ]);
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
export PATH="$HOME/.nvm/versions/node/v24.17.0/bin:$PATH"
node --test scripts/lib/parse-docs.test.mjs
```
Expected: FAIL — `Cannot find module './parse-docs.mjs'`.

- [ ] **Step 3: Implement `parse-docs.mjs`**

Create `scripts/lib/parse-docs.mjs`:
```javascript
import fs from "node:fs";
import path from "node:path";
import matter from "gray-matter";

export const GROUP_ORDER = [
  "Overview",
  "Getting started",
  "Configuration",
  "Concepts",
  "The visual TUI",
  "Architecture",
  "Roadmap",
];

const REQUIRED_FIELDS = ["id", "title", "group", "order"];

export function loadPages(contentDir) {
  const files = fs
    .readdirSync(contentDir)
    .filter((f) => f.endsWith(".md"))
    .sort();

  const pages = files.map((file) => {
    const raw = fs.readFileSync(path.join(contentDir, file), "utf8");
    const { data, content } = matter(raw);

    for (const field of REQUIRED_FIELDS) {
      if (data[field] === undefined || data[field] === null || data[field] === "") {
        throw new Error(`${file}: missing required frontmatter field "${field}"`);
      }
    }
    if (!GROUP_ORDER.includes(data.group)) {
      throw new Error(
        `${file}: unknown group "${data.group}" (expected one of ${GROUP_ORDER.join(", ")})`
      );
    }

    return {
      id: String(data.id),
      title: String(data.title),
      navTitle: data.navTitle ? String(data.navTitle) : String(data.title),
      group: String(data.group),
      order: Number(data.order),
      badgeText: data.badgeText ? String(data.badgeText) : null,
      badgeColor: data.badgeColor ? String(data.badgeColor) : "teal",
      body: content.trim(),
      file,
    };
  });

  const seen = new Set();
  for (const p of pages) {
    if (seen.has(p.id)) {
      throw new Error(`Duplicate page id "${p.id}" (from ${p.file})`);
    }
    seen.add(p.id);
  }

  pages.sort((a, b) => {
    const groupDiff = GROUP_ORDER.indexOf(a.group) - GROUP_ORDER.indexOf(b.group);
    if (groupDiff !== 0) return groupDiff;
    return a.order - b.order;
  });

  return pages;
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
node --test scripts/lib/parse-docs.test.mjs
```
Expected: all 8 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add scripts/lib/parse-docs.mjs scripts/lib/parse-docs.test.mjs
git commit -m "feat: add docs frontmatter parser + validation"
```

---

### Task 3: `render-docs.mjs` — markdown → styled HTML

**Files:**
- Create: `scripts/lib/render-docs.mjs`
- Create: `scripts/lib/render-docs.test.mjs`

This module post-processes `marked`'s default HTML output (stable, version-independent shape: `<table>`, `<pre><code class="language-X">`, `<code>`, `<h2>`) into the site's existing CSS classes, rather than relying on `marked`'s internal Renderer token API (which varies across versions). It also pre-processes a `> [!NOTE]` / `[!TIP]` / `[!WARNING]` blockquote shorthand into the site's `.callout-*` markup before handing the body to `marked`.

- [ ] **Step 1: Write the failing tests**

Create `scripts/lib/render-docs.test.mjs`:
```javascript
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
node --test scripts/lib/render-docs.test.mjs
```
Expected: FAIL — `Cannot find module './render-docs.mjs'`.

- [ ] **Step 3: Implement `render-docs.mjs`**

Create `scripts/lib/render-docs.mjs`:
```javascript
import { marked } from "marked";

const CALLOUT_RE = /^> \[!(NOTE|TIP|WARNING)\]\n((?:> ?.*\n?)+)/gm;

export function transformCallouts(markdown) {
  return markdown.replace(CALLOUT_RE, (_match, kind, body) => {
    const text = body
      .split("\n")
      .filter((l) => l.length > 0)
      .map((l) => l.replace(/^> ?/, ""))
      .join(" ")
      .trim();
    const cls = kind.toLowerCase();
    const label = kind[0] + kind.slice(1).toLowerCase();
    const inline = marked.parseInline(text).replace(/<code>/g, '<code class="ic">');
    return (
      `<div class="callout callout-${cls}"><div><div class="callout-label">${label}</div>` +
      `<div class="callout-body">${inline}</div></div></div>\n\n`
    );
  });
}

export function styleCodeBlocks(html) {
  return html.replace(
    /<pre><code class="language-([^"]*)">([\s\S]*?)<\/code><\/pre>/g,
    (_match, lang, escapedCode) => {
      const dataCopy = escapedCode.replace(/"/g, "&quot;");
      return (
        `<div class="codeblock"><div class="codeblock-bar"><span>${lang}</span>` +
        `<button class="copy-btn" data-copy="${dataCopy}">copy</button></div>` +
        `<pre><code>${escapedCode}</code></pre></div>`
      );
    }
  );
}

export function styleTables(html) {
  return html.replace(/<table>([\s\S]*?)<\/table>/g, (_tableMatch, inner) => {
    const styled = inner.replace(/<tbody>([\s\S]*?)<\/tbody>/, (_tbodyMatch, tbodyInner) => {
      const rows = tbodyInner.replace(/<tr>([\s\S]*?)<\/tr>/g, (_rowMatch, rowInner) => {
        const cellClasses = ["key", "type", "desc"];
        let i = 0;
        const styledRow = rowInner.replace(/<td>([\s\S]*?)<\/td>/g, (_cellMatch, cellContent) => {
          const cls = cellClasses[i] || "";
          i++;
          return `<td class="${cls}">${cellContent}</td>`;
        });
        return `<tr>${styledRow}</tr>`;
      });
      return `<tbody>${rows}</tbody>`;
    });
    return `<table class="key-table">${styled}</table>`;
  });
}

export function styleInlineCode(html) {
  const pres = [];
  const stashed = html.replace(/<pre>[\s\S]*?<\/pre>/g, (m) => {
    pres.push(m);
    return `@@PRE${pres.length - 1}@@`;
  });
  const styled = stashed.replace(/<code>/g, '<code class="ic">');
  return styled.replace(/@@PRE(\d+)@@/g, (_m, i) => pres[Number(i)]);
}

export function addHeadingIds(html, pageId) {
  return html.replace(/<h2>([\s\S]*?)<\/h2>/g, (_m, text) => {
    const plain = text.replace(/<[^>]+>/g, "");
    const slug = plain
      .toLowerCase()
      .trim()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/(^-|-$)/g, "");
    return `<h2 id="${pageId}-${slug}">${text}</h2>`;
  });
}

export function renderPageBody(page) {
  const withCallouts = transformCallouts(page.body);
  let html = marked.parse(withCallouts);
  html = styleCodeBlocks(html);
  html = styleTables(html);
  html = styleInlineCode(html);
  html = addHeadingIds(html, page.id);
  return html;
}

function escapeHtml(str) {
  return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function renderPageHtml(page, isFirst) {
  const badge = page.badgeText
    ? `<span class="badge badge-${page.badgeColor}" style="margin-bottom:12px">${escapeHtml(
        page.badgeText
      )}</span>\n      `
    : "";
  const body = renderPageBody(page);
  const hiddenAttr = isFirst ? "" : " hidden";
  return (
    `    <div data-page="${page.id}"${hiddenAttr}>\n` +
    `      ${badge}<h1>${escapeHtml(page.title)}</h1>\n` +
    `      <button class="copy-btn copy-page-btn" data-copy-page="${page.id}">Copy page</button>\n` +
    `      ${body}\n` +
    `    </div>`
  );
}

export function renderNavHtml(pages, groupOrder) {
  const byGroup = new Map();
  for (const g of groupOrder) byGroup.set(g, []);
  for (const p of pages) byGroup.get(p.group).push(p);

  const groups = groupOrder
    .filter((g) => byGroup.get(g).length > 0)
    .map((g) => {
      const items = byGroup
        .get(g)
        .slice()
        .sort((a, b) => a.order - b.order)
        .map(
          (p) =>
            `      <a class="nav-item" href="#${p.id}" data-nav-item="${p.id}">${escapeHtml(
              p.navTitle
            )}</a>`
        )
        .join("\n");
      return (
        `    <div class="nav-group" data-nav-group="${escapeHtml(g)}">\n` +
        `      <div class="nav-group-label">${escapeHtml(g)}</div>\n${items}\n    </div>`
      );
    });
  return groups.join("\n");
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
node --test scripts/lib/render-docs.test.mjs
```
Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
git add scripts/lib/render-docs.mjs scripts/lib/render-docs.test.mjs
git commit -m "feat: add markdown-to-HTML rendering for docs pages"
```

---

### Task 4: `render-llms.mjs` — llms.txt / llms-full.txt generation

**Files:**
- Create: `scripts/lib/render-llms.mjs`
- Create: `scripts/lib/render-llms.test.mjs`

- [ ] **Step 1: Write the failing tests**

Create `scripts/lib/render-llms.test.mjs`:
```javascript
import test from "node:test";
import assert from "node:assert/strict";
import { renderLlmsTxt, renderLlmsFullTxt } from "./render-llms.mjs";

const PAGES = [
  { id: "what-is", title: "What is entheai", navTitle: "What is entheai", group: "Overview", order: 1, body: "entheai is a hybrid coding agent. It does many things." },
  { id: "who", title: "Who it's for", navTitle: "Who it's for", group: "Overview", order: 2, body: "Solo developers on Apple Silicon." },
  { id: "install", title: "Install & build", navTitle: "Install & build", group: "Getting started", order: 1, body: "Clone the repo and build it.\n\n```bash\ncargo build\n```" },
];
const GROUP_ORDER = ["Overview", "Getting started", "Configuration", "Concepts", "The visual TUI", "Architecture", "Roadmap"];

test("renderLlmsTxt includes title, summary blockquote, and one section per group", () => {
  const out = renderLlmsTxt(PAGES, GROUP_ORDER, {
    siteTitle: "entheai",
    summary: "A hybrid coding agent.",
    baseUrl: "https://entheai.com",
  });
  assert.match(out, /^# entheai\n/);
  assert.match(out, /> A hybrid coding agent\./);
  assert.match(out, /## Overview/);
  assert.match(out, /## Getting started/);
  assert.match(out, /\[What is entheai\]\(https:\/\/entheai\.com\/docs\/#what-is\)/);
});

test("renderLlmsTxt link description is the first sentence, stripped of code fences and markup", () => {
  const out = renderLlmsTxt(PAGES, GROUP_ORDER, {
    siteTitle: "entheai",
    summary: "s",
    baseUrl: "https://entheai.com",
  });
  assert.match(out, /entheai is a hybrid coding agent\./);
  assert.doesNotMatch(out, /```/);
});

test("renderLlmsTxt omits groups with no pages", () => {
  const out = renderLlmsTxt(PAGES, GROUP_ORDER, {
    siteTitle: "entheai",
    summary: "s",
    baseUrl: "https://entheai.com",
  });
  assert.doesNotMatch(out, /## Architecture/);
});

test("renderLlmsFullTxt concatenates all pages with title headings and separators", () => {
  const out = renderLlmsFullTxt(PAGES);
  assert.match(out, /## What is entheai/);
  assert.match(out, /## Who it's for/);
  assert.match(out, /## Install & build/);
  assert.match(out, /entheai is a hybrid coding agent\. It does many things\./);
  assert.match(out, /\n---\n/);
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
node --test scripts/lib/render-llms.test.mjs
```
Expected: FAIL — `Cannot find module './render-llms.mjs'`.

- [ ] **Step 3: Implement `render-llms.mjs`**

Create `scripts/lib/render-llms.mjs`:
```javascript
function firstSentence(markdownBody) {
  const plain = markdownBody
    .replace(/```[\s\S]*?```/g, "")
    .replace(/^>.*$/gm, "")
    .replace(/[#*_`]/g, "")
    .replace(/\s+/g, " ")
    .trim();
  const match = plain.match(/^[^.!?]*[.!?]/);
  return (match ? match[0] : plain.slice(0, 140)).trim();
}

export function renderLlmsTxt(pages, groupOrder, opts) {
  const { siteTitle, summary, baseUrl } = opts;
  const byGroup = new Map();
  for (const g of groupOrder) byGroup.set(g, []);
  for (const p of pages) byGroup.get(p.group).push(p);

  const sections = groupOrder
    .filter((g) => byGroup.get(g).length > 0)
    .map((g) => {
      const links = byGroup
        .get(g)
        .slice()
        .sort((a, b) => a.order - b.order)
        .map((p) => `- [${p.navTitle}](${baseUrl}/docs/#${p.id}): ${firstSentence(p.body)}`)
        .join("\n");
      return `## ${g}\n\n${links}`;
    })
    .join("\n\n");

  return `# ${siteTitle}\n\n> ${summary}\n\n${sections}\n`;
}

export function renderLlmsFullTxt(pages) {
  return (
    pages.map((p) => `## ${p.title}\n\n${p.body.trim()}`).join("\n\n---\n\n") + "\n"
  );
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
node --test scripts/lib/render-llms.test.mjs
```
Expected: all tests PASS.

- [ ] **Step 5: Run the full test suite and commit**

```bash
npm test
git add scripts/lib/render-llms.mjs scripts/lib/render-llms.test.mjs
git commit -m "feat: add llms.txt / llms-full.txt generation"
```

---

### Task 5: `scripts/build-site.mjs` — orchestrator

**Files:**
- Create: `scripts/build-site.mjs`
- Create: `scripts/build-site.test.mjs`

- [ ] **Step 1: Write the failing test**

Create `scripts/build-site.test.mjs` (exercises the orchestrator end-to-end against a fixture directory tree, not the real `public/`):
```javascript
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
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
node --test scripts/build-site.test.mjs
```
Expected: FAIL — `Cannot find module './build-site.mjs'`.

- [ ] **Step 3: Implement `build-site.mjs`**

Create `scripts/build-site.mjs`:
```javascript
#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { loadPages, GROUP_ORDER } from "./lib/parse-docs.mjs";
import { renderPageHtml, renderNavHtml } from "./lib/render-docs.mjs";
import { renderLlmsTxt, renderLlmsFullTxt } from "./lib/render-llms.mjs";

const DEFAULT_BASE_URL = "https://entheai.com";
const DEFAULT_SITE_TITLE = "entheai";
const DEFAULT_SITE_SUMMARY =
  "entheai is a personal, macOS/Apple-Silicon, terminal-native hybrid coding agent written in Rust — a cloud orchestrator plans, then fans out to model-matched sub-agents that build in parallel and merge back verified.";

export function build({
  root,
  baseUrl = DEFAULT_BASE_URL,
  siteTitle = DEFAULT_SITE_TITLE,
  siteSummary = DEFAULT_SITE_SUMMARY,
} = {}) {
  const publicDir = path.join(root, "public");
  const contentDir = path.join(publicDir, "docs", "content");
  const templatePath = path.join(publicDir, "docs", "_template.html");
  const outputDocsHtml = path.join(publicDir, "docs", "index.html");
  const outputLlmsTxt = path.join(publicDir, "llms.txt");
  const outputLlmsFullTxt = path.join(publicDir, "llms-full.txt");

  const pages = loadPages(contentDir);

  const pagesHtml = pages.map((p, i) => renderPageHtml(p, i === 0)).join("\n\n");
  const navHtml = renderNavHtml(pages, GROUP_ORDER);
  const mdScripts = pages
    .map((p) => {
      const safeBody = p.body.replace(/<\/script/gi, "<\\/script");
      return `<script type="text/markdown" id="md-${p.id}">${safeBody}</script>`;
    })
    .join("\n");

  let template = fs.readFileSync(templatePath, "utf8");
  if (!template.includes("<!--NAV-->")) throw new Error("_template.html missing <!--NAV--> marker");
  if (!template.includes("<!--PAGES-->")) throw new Error("_template.html missing <!--PAGES--> marker");
  if (!template.includes("<!--MD_SCRIPTS-->"))
    throw new Error("_template.html missing <!--MD_SCRIPTS--> marker");

  template = template
    .replace("<!--NAV-->", navHtml)
    .replace("<!--PAGES-->", pagesHtml)
    .replace("<!--MD_SCRIPTS-->", mdScripts);

  fs.writeFileSync(outputDocsHtml, template);

  const llmsTxt = renderLlmsTxt(pages, GROUP_ORDER, { siteTitle, summary: siteSummary, baseUrl });
  fs.writeFileSync(outputLlmsTxt, llmsTxt);

  const llmsFullTxt = renderLlmsFullTxt(pages);
  fs.writeFileSync(outputLlmsFullTxt, llmsFullTxt);

  return { pageCount: pages.length, outputDocsHtml, outputLlmsTxt, outputLlmsFullTxt };
}

function main() {
  const __dirname = path.dirname(fileURLToPath(import.meta.url));
  const root = path.join(__dirname, "..");
  const result = build({ root });
  console.log(`Built ${result.pageCount} docs pages -> ${result.outputDocsHtml}`);
  console.log(`Wrote ${result.outputLlmsTxt}`);
  console.log(`Wrote ${result.outputLlmsFullTxt}`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main();
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
node --test scripts/build-site.test.mjs
npm test
```
Expected: all tests PASS (this also re-runs Tasks 2-4's tests).

- [ ] **Step 5: Commit**

```bash
git add scripts/build-site.mjs scripts/build-site.test.mjs
git commit -m "feat: add build-site orchestrator"
```

---

### Task 6: Extract `public/docs/_template.html`

**Files:**
- Create: `public/docs/_template.html`
- Modify: `.gitignore`

- [ ] **Step 1: Create the template**

Create `public/docs/_template.html` — this is `public/docs/index.html` (as it exists today) with the hardcoded `<nav class="sidebar">` group markup replaced by `<!--NAV-->`, the hardcoded `<div data-page="...">` blocks replaced by `<!--PAGES-->`, a `<!--MD_SCRIPTS-->` marker added before `</body>`, the `[data-page] p b` CSS selector changed to `[data-page] p strong` (markdown bold renders `<strong>`, not `<b>`), and a `.copy-page-btn` spacing rule added:

```html
<!DOCTYPE html>
<html lang="en" data-theme="dark">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>entheai docs</title>
<meta name="description" content="entheai documentation — installation, configuration, the agent loop, the tiered router, fan-out, permissions, memory, and architecture.">
<link rel="stylesheet" href="../assets/css/tokens.css">
<style>
  body { color: var(--text-primary); }
  header.topbar {
    position: sticky; top: 0; z-index: 30; height: var(--topbar-h);
    display: flex; align-items: center; gap: 16px; padding: 0 var(--gutter);
    background: color-mix(in oklab, var(--bg-app) 82%, transparent);
    backdrop-filter: blur(12px); border-bottom: 1px solid var(--border-subtle);
  }
  .burger { display: none; background: none; border: none; color: var(--text-primary); font-size: 20px; cursor: pointer; }
  .brand { font-family: var(--font-mono); font-weight: 700; font-size: var(--text-lg); letter-spacing: -0.03em; color: var(--text-primary); }
  .brand .sub { color: var(--text-faint); font-weight: 400; font-size: var(--text-sm); }
  .search-wrap { flex: 1; max-width: 320px; position: relative; margin-left: auto; }
  .search-wrap input {
    width: 100%; padding: 7px 12px; font-family: var(--font-mono); font-size: var(--text-sm);
    background: var(--bg-inset); border: 1px solid var(--border-default); border-radius: var(--radius-md);
    color: var(--text-primary); outline: none;
  }
  .search-results {
    position: absolute; top: 110%; left: 0; right: 0; background: var(--bg-raised);
    border: 1px solid var(--border-default); border-radius: var(--radius-md); box-shadow: var(--shadow-lg);
    overflow: hidden; max-height: 300px; overflow-y: auto; z-index: 40;
  }
  .search-hit {
    display: block; width: 100%; text-align: left; padding: 9px 12px; background: none; border: none;
    cursor: pointer; font-family: var(--font-mono); font-size: var(--text-sm); color: var(--text-secondary);
  }
  .search-hit-group { color: var(--text-faint); margin-left: 8px; font-size: var(--text-xs); }
  .search-empty { padding: 9px 12px; color: var(--text-faint); font-family: var(--font-mono); font-size: var(--text-sm); }
  .theme-toggle {
    background: var(--bg-inset); border: 1px solid var(--border-default); border-radius: var(--radius-md);
    color: var(--text-secondary); width: 34px; height: 34px; cursor: pointer; font-size: 15px;
  }
  .gh-link { font-family: var(--font-mono); font-size: var(--text-sm); }

  .docs-grid {
    display: grid; grid-template-columns: var(--sidebar-w) minmax(0, 1fr) var(--toc-w);
    align-items: start; max-width: var(--container-wide); margin: 0 auto;
  }
  nav.sidebar { padding: 20px 14px; font-family: var(--font-mono); position: sticky; top: var(--topbar-h);
    height: calc(100vh - var(--topbar-h)); overflow-y: auto; border-right: 1px solid var(--border-subtle); }
  .nav-group { margin-bottom: 20px; }
  .nav-group-label {
    font-size: var(--text-xs); text-transform: uppercase; letter-spacing: var(--tracking-caps);
    color: var(--text-faint); padding: 0 10px 8px;
  }
  .nav-item {
    display: block; width: 100%; text-align: left; padding: 7px 10px; margin-bottom: 1px;
    font-family: var(--font-mono); font-size: var(--text-sm); cursor: pointer; border-radius: var(--radius-sm);
    border: none; border-left: 2px solid transparent; background: transparent; color: var(--text-muted);
    transition: color var(--dur-fast) var(--ease-out), background var(--dur-fast) var(--ease-out);
    text-decoration: none;
  }
  .nav-item:hover { color: var(--text-secondary); }
  .nav-item.is-active {
    background: color-mix(in oklab, var(--teal-400) 12%, transparent);
    color: var(--teal-300); border-left-color: var(--teal-400);
  }

  main.docs-main { padding: 40px clamp(24px, 4vw, 64px) 80px; min-height: 70vh; }
  [data-page] { animation: ent-docfade var(--dur-base) var(--ease-out); }
  [data-page] h1 { font: var(--font-h1); color: var(--text-primary); margin: 0 0 12px; }
  [data-page] h2 { font: var(--font-h2); color: var(--text-primary); margin: 40px 0 14px; scroll-margin-top: 80px; }
  [data-page] p { font: var(--font-body-lg); color: var(--text-secondary); margin: 0 0 16px; max-width: 680px; }
  [data-page] p strong { color: var(--text-primary); }
  .copy-page-btn { display: inline-block; margin: 0 0 20px; }

  .key-table { width: 100%; border-collapse: collapse; margin: 6px 0 20px; font-family: var(--font-mono); font-size: var(--text-sm); }
  .key-table th {
    text-align: left; padding: 9px 12px; color: var(--text-muted); font-weight: 600;
    border-bottom: 1px solid var(--border-default); font-size: var(--text-xs); text-transform: uppercase; letter-spacing: 0.06em;
  }
  .key-table td { padding: 9px 12px; border-bottom: 1px solid var(--border-subtle); }
  .key-table td.key { color: var(--teal-300); }
  .key-table td.type { color: var(--magenta-300); }
  .key-table td.desc { color: var(--text-secondary); font-family: var(--font-sans); }

  .doc-nav-links { display: flex; justify-content: space-between; gap: 16px; margin-top: 64px; padding-top: 24px; border-top: 1px solid var(--border-subtle); }
  .doc-nav-btn {
    background: var(--bg-raised); border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    padding: 12px 16px; cursor: pointer; font-family: var(--font-mono); font-size: var(--text-sm); max-width: 260px;
    text-decoration: none; display: block; color: inherit;
  }
  .doc-nav-btn .eyebrow-label { color: var(--text-faint); font-size: var(--text-xs); display: block; }
  .doc-nav-btn div[data-label] { color: var(--text-primary); }
  .doc-nav-btn.next { text-align: right; }
  .doc-nav-btn[hidden] { display: none; }

  aside.toc { position: sticky; top: var(--topbar-h); padding: 44px 20px; font-family: var(--font-mono); font-size: var(--text-xs); }
  .toc-label { color: var(--text-faint); text-transform: uppercase; letter-spacing: var(--tracking-caps); margin-bottom: 12px; }

  .drawer-overlay { position: fixed; inset: 0; z-index: 40; background: rgba(0,0,0,.5); }
  .drawer-panel { position: absolute; top: 0; left: 0; bottom: 0; width: 280px; background: var(--bg-app); border-right: 1px solid var(--border-default); overflow-y: auto; }

  @media (max-width: 1080px) {
    aside.toc { display: none; }
    .docs-grid { grid-template-columns: var(--sidebar-w) minmax(0, 1fr); }
  }
  @media (max-width: 760px) {
    nav.sidebar { display: none; }
    .docs-grid { grid-template-columns: minmax(0, 1fr); }
    .burger { display: block !important; }
  }
</style>
</head>
<body>

<header class="topbar">
  <button class="burger" data-burger>☰</button>
  <div class="brand">entheai <span class="sub">docs</span></div>
  <div class="search-wrap">
    <input type="text" placeholder="Search…" data-search-input>
    <div class="search-results" data-search-results hidden></div>
  </div>
  <button class="theme-toggle" data-theme-toggle title="Toggle theme">☾</button>
  <a class="gh-link" href="https://github.com/peterlodri-sec/entheai" target="_blank" rel="noopener">GitHub ↗</a>
</header>

<div class="docs-grid">
  <nav class="sidebar" id="sidebar-nav">
<!--NAV-->
  </nav>

  <main class="docs-main">

<!--PAGES-->

    <div class="doc-nav-links">
      <a class="doc-nav-btn prev" href="#" data-nav-prev hidden>
        <span class="eyebrow-label">← Previous</span>
        <div data-label></div>
      </a>
      <a class="doc-nav-btn next" href="#" data-nav-next hidden>
        <span class="eyebrow-label">Next →</span>
        <div data-label></div>
      </a>
    </div>

  </main>

  <aside class="toc">
    <div class="toc-label">On this page</div>
    <div data-toc></div>
  </aside>
</div>

<div class="drawer-overlay" data-drawer hidden>
  <div class="drawer-panel" id="drawer-nav"></div>
</div>

<script>
  document.getElementById("drawer-nav").innerHTML = document.getElementById("sidebar-nav").innerHTML;
</script>
<script src="../assets/js/docs.js"></script>
<!--MD_SCRIPTS-->
</body>
</html>
```

- [ ] **Step 2: Add generated files to `.gitignore`**

Edit `.gitignore`, add:
```
public/docs/index.html
public/llms.txt
public/llms-full.txt
```

- [ ] **Step 3: Stop tracking the now-generated `public/docs/index.html`**

Run:
```bash
git rm --cached public/docs/index.html
```
(Keep the file on disk for now — Task 9 regenerates it via the build.)

- [ ] **Step 4: Commit**

```bash
git add public/docs/_template.html .gitignore
git commit -m "refactor: extract docs shell into _template.html, stop tracking generated index.html"
```

---

### Task 7: Migrate all 17 doc pages to `public/docs/content/*.md`

**Files:**
- Create: `public/docs/content/what-is.md`
- Create: `public/docs/content/concept.md`
- Create: `public/docs/content/who.md`
- Create: `public/docs/content/install.md`
- Create: `public/docs/content/configure.md`
- Create: `public/docs/content/first-run.md`
- Create: `public/docs/content/providers.md`
- Create: `public/docs/content/models.md`
- Create: `public/docs/content/loop.md`
- Create: `public/docs/content/router.md`
- Create: `public/docs/content/fanout.md`
- Create: `public/docs/content/permissions.md`
- Create: `public/docs/content/memory.md`
- Create: `public/docs/content/extend.md`
- Create: `public/docs/content/tui.md`
- Create: `public/docs/content/arch.md`
- Create: `public/docs/content/roadmap.md`

- [ ] **Step 1: Create `public/docs/content/what-is.md`**

```markdown
---
id: what-is
title: What is entheai
group: Overview
order: 1
badgeText: Overview
---

**entheai** is a personal, macOS/Apple‑Silicon, terminal‑native hybrid coding agent written in Rust. A cloud orchestrator plans; it fans out to a swarm of model‑matched sub‑agents that build in parallel and merge back verified.

> [!NOTE]
> entheai targets a single developer on one Mac. It is not a hosted, multi‑tenant service.

## Highlights

Runs local models via `Osaurus`, ships a visual TUI with shader backgrounds and a live 3D codebase graph, and keeps compounding memory across sessions.
```

- [ ] **Step 2: Create `public/docs/content/concept.md`**

```markdown
---
id: concept
title: "Hybrid brain & fan-out"
group: Overview
order: 2
---

The **tiered hybrid brain** separates planning from execution. A capable cloud model reasons about the whole task; cheaper local or specialized models do the parallel work.

## Fan-out

The orchestrator decomposes a task into units and dispatches a sub‑agent per unit — each in its own git worktree, each on the model that best fits its role. Work merges back only after that unit's tests pass.

```text
task ──▶ orchestrator (deepseek/v4-pro)
           ├─ coder    · osaurus/qwen2.5-coder
           ├─ test     · deepseek/v4-pro
           └─ review   · osaurus/deepseek-r1
                         ▼
              merge + verify ▶ main
```
```

- [ ] **Step 3: Create `public/docs/content/who.md`**

```markdown
---
id: who
title: "Who it's for"
group: Overview
order: 3
---

Solo developers on Apple Silicon who want an agent that lives in the terminal, respects local compute, and can parallelize large refactors without a cloud bill for every token.
```

- [ ] **Step 4: Create `public/docs/content/install.md`**

```markdown
---
id: install
title: "Install & build"
group: "Getting started"
order: 1
badgeText: "Getting started"
---

Clone the repo and build a release binary. You'll need a recent Rust toolchain.

```bash
git clone https://github.com/peterlodri-sec/entheai
cd entheai
cargo build --release

./target/release/entheai --version
```

> [!TIP]
> Add `target/release` to your `PATH` so you can call `entheai` from anywhere.
```

- [ ] **Step 5: Create `public/docs/content/configure.md`**

```markdown
---
id: configure
title: "Configure entheai.toml"
group: "Getting started"
order: 2
---

entheai reads `entheai.toml` from the working directory. Point it at a local Osaurus model or a cloud provider.

```entheai.toml
[provider.osaurus]
endpoint = "http://127.0.0.1:11434"

[provider.opencode-zen]
api_key = "env:OPENCODE_ZEN_KEY"

[router]
plan = "deepseek/v4-pro"
code = "osaurus/qwen2.5-coder"
```

## Core keys

| Key | Type | Description |
|---|---|---|
| router.plan | model id | Model used for planning / orchestration. |
| router.code | model id | Default model for coder sub-agents. |
| fanout.max | int | Max parallel worktrees (default 4). |
| permissions.mode | enum | ask · yolo |
```

- [ ] **Step 6: Create `public/docs/content/first-run.md`**

```markdown
---
id: first-run
title: "First run"
group: "Getting started"
order: 3
---

Ask entheai to summarize your repo. Tokens stream into the terminal as the plan comes back.

```bash
entheai "summarize this repo"
```

> [!NOTE]
> The first run indexes your codebase into memory. Subsequent runs are faster and more context‑aware.
```

- [ ] **Step 7: Create `public/docs/content/providers.md`**

```markdown
---
id: providers
title: Providers
group: Configuration
order: 1
badgeText: Configuration
---

entheai speaks to several model backends. Local‑first via Osaurus; cloud when you need heavier reasoning.

| Key | Type | Description |
|---|---|---|
| osaurus | local | On-device models on Apple Silicon. |
| opencode-zen | cloud | Hosted open models. |
| deepseek | cloud | DeepSeek V4 Pro — planning tier. |
| openrouter | cloud | Router to many providers. |
```

- [ ] **Step 8: Create `public/docs/content/models.md`**

```markdown
---
id: models
title: "Models & ids"
group: Configuration
order: 2
---

Every model is referenced with the `<provider>/<model>` convention.

```text
osaurus/qwen2.5-coder
opencode-zen/scribe
deepseek/v4-pro
openrouter/anthropic/claude
```

> [!WARNING]
> If a provider is unavailable at runtime, the router falls back to the next tier and logs the substitution.
```

- [ ] **Step 9: Create `public/docs/content/loop.md`**

```markdown
---
id: loop
title: "The agent loop"
group: Concepts
order: 1
badgeText: Concepts
---

Perceive → plan → act → verify. Each tool call passes through the permission gate; each result folds back into memory.
```

- [ ] **Step 10: Create `public/docs/content/router.md`**

```markdown
---
id: router
title: "The tiered router"
group: Concepts
order: 2
---

The router matches each unit of work to a model tier by difficulty, cost, and locality — planning to the strongest model, mechanical edits to fast local ones.
```

- [ ] **Step 11: Create `public/docs/content/fanout.md`**

```markdown
---
id: fanout
title: "Fan-out & sub-agent roles"
navTitle: "Fan-out & roles"
group: Concepts
order: 3
---

Roles include `coder`, `docs`, `test`, and `review`. Each runs in an isolated worktree so parallel work never collides.
```

- [ ] **Step 12: Create `public/docs/content/permissions.md`**

```markdown
---
id: permissions
title: "Permission gate & YOLO"
group: Concepts
order: 4
---

By default every side‑effecting tool call asks for approval.

```text
allow run_shell("cargo test")? [y/N]
```

> [!WARNING]
> YOLO mode (`--yolo`) auto‑approves every tool call. Use it only in a sandbox or a throwaway worktree.
```

- [ ] **Step 13: Create `public/docs/content/memory.md`**

```markdown
---
id: memory
title: "Memory — five namespaces"
navTitle: "Memory namespaces"
group: Concepts
order: 5
---

Compounding memory with auto‑compaction keeps context lean while retaining what matters.

| Key | Type | Description |
|---|---|---|
| codebase | index | Symbols, files, and the dependency graph. |
| session | short | The current conversation. |
| project | long | Decisions and conventions for this repo. |
| user | long | Your global preferences. |
| skills | meta | Learned procedures and playbooks. |
```

- [ ] **Step 14: Create `public/docs/content/extend.md`**

```markdown
---
id: extend
title: "Skills · plugins · MCP"
group: Concepts
order: 6
---

Bundle reusable procedures as `skills`, add capabilities with `plugins`, and connect external tools over `MCP`.
```

- [ ] **Step 15: Create `public/docs/content/tui.md`**

```markdown
---
id: tui
title: "Shader & codebase graph"
group: "The visual TUI"
order: 1
badgeText: "Visual TUI"
badgeColor: magenta
---

The TUI renders an animated shader background and can toggle a live 3D graph of your codebase — nodes are modules, edges are dependencies, lit as the agent touches them.

| Key | Type | Description |
|---|---|---|
| g | key | Toggle the 3D codebase graph. |
| f | key | Cycle shader flicker / calm. |
| tab | key | Move focus between panes. |
```

- [ ] **Step 16: Create `public/docs/content/arch.md`**

```markdown
---
id: arch
title: "Crate map & system"
group: Architecture
order: 1
badgeText: Architecture
---

entheai is a Rust workspace. The orchestrator drives providers, Osaurus, a codebase‑memory MCP server, and the sub‑agent pool.

```text
entheai-core      · agent loop, router
entheai-providers · osaurus, zen, deepseek
entheai-memory    · MCP server, 5 namespaces
entheai-tui       · shader + codebase graph
entheai-agents    · worktree pool, merge/verify
```
```

- [ ] **Step 17: Create `public/docs/content/roadmap.md`**

```markdown
---
id: roadmap
title: "Roadmap & design docs"
group: Roadmap
order: 1
---

Longer‑form specs and plans live alongside the source.

<a class="btn btn-ghost" href="https://github.com/peterlodri-sec/entheai" target="_blank" rel="noopener">Design docs on GitHub <span>↗</span></a>
```

- [ ] **Step 18: Commit**

```bash
git add public/docs/content
git commit -m "content: migrate all doc pages to markdown source"
```

---

### Task 8: "Copy page" button behavior in `docs.js`

**Files:**
- Modify: `public/assets/js/docs.js`

- [ ] **Step 1: Add the copy-page-button handler**

In `public/assets/js/docs.js`, find the existing block:
```javascript
  document.querySelectorAll("[data-copy]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const text = btn.getAttribute("data-copy");
      navigator.clipboard.writeText(text).catch(() => {});
      const label = btn.textContent;
      btn.textContent = "copied ✓";
      btn.classList.add("is-copied");
      setTimeout(() => {
        btn.textContent = label;
        btn.classList.remove("is-copied");
      }, 1400);
    });
  });
```
Immediately after that block (still inside the module's top-level IIFE, before the final `const initial = ...` lines), add:
```javascript
  document.querySelectorAll("[data-copy-page]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const id = btn.getAttribute("data-copy-page");
      const src = document.getElementById("md-" + id);
      if (!src) return;
      navigator.clipboard.writeText(src.textContent).catch(() => {});
      const label = btn.textContent;
      btn.textContent = "copied ✓";
      btn.classList.add("is-copied");
      setTimeout(() => {
        btn.textContent = label;
        btn.classList.remove("is-copied");
      }, 1400);
    });
  });
```

- [ ] **Step 2: Commit**

```bash
git add public/assets/js/docs.js
git commit -m "feat: wire up per-page Copy page button"
```

---

### Task 9: Wire up the build, generate output, verify no drift

**Files:**
- Modify: `public/docs/index.html` (generated — will be re-created, not hand-edited)
- Modify: `public/llms.txt`, `public/llms-full.txt` (generated, new)

- [ ] **Step 1: Run the full test suite**

Run:
```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
export PATH="$HOME/.nvm/versions/node/v24.17.0/bin:$PATH"
npm test
```
Expected: all tests across `parse-docs.test.mjs`, `render-docs.test.mjs`, `render-llms.test.mjs`, `build-site.test.mjs` PASS.

- [ ] **Step 2: Run the real build**

Run:
```bash
npm run build
```
Expected output: `Built 17 docs pages -> .../public/docs/index.html`, plus two `Wrote ...` lines for `llms.txt` and `llms-full.txt`.

- [ ] **Step 3: Sanity-check the generated files**

Run:
```bash
grep -c 'data-page="' public/docs/index.html
grep -c 'data-nav-item="' public/docs/index.html
head -20 public/llms.txt
wc -l public/llms-full.txt
```
Expected: 17 `data-page="` matches, 17 `data-nav-item="` matches, `llms.txt` starts with `# entheai`, `llms-full.txt` is non-trivially sized (contains all 17 pages' bodies).

- [ ] **Step 4: No commit needed**

These three files are gitignored build artifacts (per Task 6) — nothing to commit here. If `git status` shows them as tracked/modified, re-check Task 6 Step 2/3 were applied correctly.

---

### Task 10: Browser verification

**Files:** none (verification only)

- [ ] **Step 1: Serve `public/` locally**

Run in the background:
```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai/public
python3 -m http.server 8935
```

- [ ] **Step 2: Verify the docs page loads with all content**

Using a browser tool, navigate to `http://localhost:8935/docs/` and confirm:
- Sidebar shows all 7 groups and 17 pages in the same order as before
- The "What is entheai" page is active by default, shows the teal "Overview" badge, the note callout, and the "Highlights" heading
- Clicking "Configure entheai.toml" in the sidebar shows the `entheai.toml` code block (bar text reads `entheai.toml`, not a language name) and the "Core keys" table with teal/magenta/secondary-colored columns
- The new "Copy page" button appears under each page's `<h1>` and, when clicked, copies that page's raw markdown (paste somewhere to confirm) and shows "copied ✓" briefly
- Existing inline `data-copy` buttons (on code blocks) still work as before
- Search, theme toggle, TOC (on the "Configure" page, should show "Core keys"), prev/next, and the mobile drawer (resize to <760px) all still work exactly as before

- [ ] **Step 3: Spot-check `llms.txt` and `llms-full.txt` in the browser**

Navigate to `http://localhost:8935/llms.txt` and `http://localhost:8935/llms-full.txt`; confirm both render as plain text with sensible content (not empty, not raw unrendered markdown artifacts like stray `###`-only lines).

- [ ] **Step 4: Stop the local server**

```bash
pkill -f "http.server 8935"
```

---

### Task 11: `public/AGENTS.md`

**Files:**
- Create: `public/AGENTS.md`

- [ ] **Step 1: Write the file**

Create `public/AGENTS.md`:
```markdown
# AGENTS.md — entheai.com (public/)

> Frontend for the entheai landing + docs site. For the Rust CLI this site documents, see the repo-root `AGENTS.md`.

## What lives here

- `public/index.html` — the landing page. Hand-authored HTML/CSS/JS. Edit it directly.
- `public/404.html` — hand-authored, edit directly.
- `public/assets/` — shared design tokens (`css/tokens.css`) and JS (`shader-field.js`, `reveal.js`, `landing.js`, `docs.js`). Edit directly.
- `public/docs/content/*.md` — the docs pages' **source of truth**. One file per page, with YAML frontmatter (`id`, `title`, `group`, `order`, optional `navTitle`, `badgeText`, `badgeColor`).
- `public/docs/_template.html` — the docs page shell (topbar, sidebar chrome, search/theme/TOC JS wiring). Edit this for structural/style changes to the docs shell itself.
- `public/docs/index.html`, `public/llms.txt`, `public/llms-full.txt` — **generated build output, gitignored**. Never hand-edit these; your changes will be silently overwritten by the next `npm run build`.

## Adding or editing a doc page

1. Create or edit a file in `public/docs/content/`. Required frontmatter: `id` (unique), `title`, `group` (must be one of: Overview, Getting started, Configuration, Concepts, The visual TUI, Architecture, Roadmap), `order` (integer, position within its group).
2. Markdown extensions supported beyond standard GFM: a `> [!NOTE]` / `> [!TIP]` / `> [!WARNING]` blockquote-prefix shorthand renders as the site's callout boxes. Tables render as the site's `.key-table` style (3 columns: Key, Type, Description).
3. Run `npm run build` and preview locally (`python3 -m http.server` from inside `public/`, then visit `/docs/`).
4. Never edit `public/docs/index.html` directly — edit the source `.md` file and rebuild.

## Design tokens

Reuse the CSS custom properties defined in `public/assets/css/tokens.css` (colors, spacing, type scale, etc.) rather than hardcoding values — both the landing page and generated docs page depend on this shared token set for the dark/light theme system to work.

## Build & deploy

- Build: `npm run build` (runs `scripts/build-site.mjs`, reads `public/docs/content/*.md`, writes the three generated files above).
- Test the build pipeline itself: `npm test` (tests live in `scripts/lib/*.test.mjs` and `scripts/build-site.test.mjs`, run via Node's built-in test runner).
- Deploy is automatic on push to `main` via `.github/workflows/deploy.yml` (path-filtered to site-relevant files). Manual deploy: `npm run build && npx wrangler deploy`.
```

- [ ] **Step 2: Commit**

```bash
git add public/AGENTS.md
git commit -m "docs: add frontend-scoped AGENTS.md"
```

---

### Task 12: Scoped Cloudflare API Token + GitHub `main` environment secret

**Files:** none (infrastructure only)

This task creates a new, narrowly-scoped Cloudflare API Token (Workers Scripts Write on this account + Workers Routes Write / DNS Write / SSL and Certificates Write on the `entheai.com` zone only) and stores it directly as the `CLOUDFLARE_API_TOKEN` secret in a GitHub Environment named `main` on `peterlodri-sec/entheai`. The permission group IDs below were looked up and confirmed against this Cloudflare account on 2026-07-18.

- [ ] **Step 1: Confirm the GitHub `main` environment exists (create if not)**

Run:
```bash
gh api repos/peterlodri-sec/entheai/environments/main -X PUT --silent
```
Expected: no output, exit code 0 (creates the environment if it doesn't already exist; idempotent if it does).

- [ ] **Step 2: Create the scoped Cloudflare API Token and store it as the GitHub environment secret**

This step reads the local `wrangler` OAuth session token (already authenticated, expires within hours, used here only as the credential to mint the new long-lived token) and never echoes either token to stdout.

Run:
```bash
set -euo pipefail

CF_ACCOUNT_ID="5b8d7905dd79e25025bcc0c8f33d5940"
CF_ZONE_ID="09a3145ea24ea5b2b821cc8283097f9b"
CF_AUTH=$(grep '^oauth_token' ~/.config/.wrangler/config/default.toml | sed -E 's/^oauth_token *= *"(.*)"$/\1/')

WORKERS_SCRIPTS_WRITE="e086da7e2179491d91ee5f35b3ca210a"
WORKERS_ROUTES_WRITE="28f4b596e7d643029c524985477ae49a"
DNS_WRITE="4755a26eedb94da69e1066d98aa820be"
SSL_CERTS_WRITE="c03055bc037c4ea9afb9a9f104b7b721"

TOKEN_JSON=$(curl -s -X POST "https://api.cloudflare.com/client/v4/user/tokens" \
  -H "Authorization: Bearer ${CF_AUTH}" \
  -H "Content-Type: application/json" \
  --data @- <<JSON
{
  "name": "entheai-site-deploy-ci",
  "policies": [
    {
      "effect": "allow",
      "resources": { "com.cloudflare.api.account.${CF_ACCOUNT_ID}": "*" },
      "permission_groups": [ { "id": "${WORKERS_SCRIPTS_WRITE}" } ]
    },
    {
      "effect": "allow",
      "resources": { "com.cloudflare.api.account.zone.${CF_ZONE_ID}": "*" },
      "permission_groups": [
        { "id": "${WORKERS_ROUTES_WRITE}" },
        { "id": "${DNS_WRITE}" },
        { "id": "${SSL_CERTS_WRITE}" }
      ]
    }
  ]
}
JSON
)

SUCCESS=$(echo "$TOKEN_JSON" | jq -r '.success')
if [ "$SUCCESS" != "true" ]; then
  echo "Token creation failed:" >&2
  echo "$TOKEN_JSON" | jq -r '.errors' >&2
  exit 1
fi

NEW_TOKEN=$(echo "$TOKEN_JSON" | jq -r '.result.value')

gh secret set CLOUDFLARE_API_TOKEN --env main --repo peterlodri-sec/entheai --body "$NEW_TOKEN"

unset NEW_TOKEN TOKEN_JSON CF_AUTH
```

Note: this command uses `curl` (required — it's calling Cloudflare's REST API directly with a Bearer token that must not be typed into any MCP tool's arguments) rather than the `cloudflare-api` MCP tool, specifically so the newly-minted token value never needs to leave this one shell process before being handed to `gh secret set`.

Expected: no error printed; the script exits 0. The full token value is never printed by this script (only `$SUCCESS` — the string `"true"` — is ever echoed).

- [ ] **Step 3: Confirm the secret exists (without revealing its value)**

Run:
```bash
gh secret list --env main --repo peterlodri-sec/entheai
```
Expected: a line showing `CLOUDFLARE_API_TOKEN` with an "Updated" timestamp. `gh secret list` never prints secret values, only names and metadata.

- [ ] **Step 4: Note on transient exposure**

The token value passed through this shell session's `TOKEN_JSON`/`NEW_TOKEN` variables and was never printed to stdout/stderr by any command above. If you want zero transient exposure even within tool-call plumbing, you can rotate this token later via the Cloudflare dashboard (Profile → API Tokens) and re-run `gh secret set CLOUDFLARE_API_TOKEN --env main --repo peterlodri-sec/entheai` with the new value — this is a normal, revocable, narrowly-scoped credential, not the broad OAuth session.

No commit needed — this task is pure infrastructure setup.

---

### Task 13: `.github/workflows/deploy.yml`

**Files:**
- Create: `.github/workflows/deploy.yml`

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/deploy.yml`:
```yaml
name: Deploy site
on:
  push:
    branches: [main]
    paths:
      - "public/**"
      - "scripts/**"
      - "wrangler.jsonc"
      - "package.json"
      - "package-lock.json"
      - ".github/workflows/deploy.yml"

jobs:
  deploy:
    runs-on: ubuntu-latest
    environment: main
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: npm
      - run: npm ci
      - run: npm run build
      - run: npm test
      - uses: cloudflare/wrangler-action@v3
        with:
          apiToken: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          command: deploy
```

Note `npm test` runs before deploy — the build-pipeline's own test suite acts as a last-chance gate so a broken renderer doesn't ship silently.

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/deploy.yml
git commit -m "ci: deploy site to Cloudflare Workers on push to main"
```

---

### Task 14: Push and verify the pipeline end-to-end

**Files:** none (verification only)

- [ ] **Step 1: Push all commits to main**

```bash
git push origin main
```

- [ ] **Step 2: Watch the workflow run**

```bash
gh run watch --repo peterlodri-sec/entheai
```
Expected: the "Deploy site" run completes with all steps green (checkout, setup-node, npm ci, npm run build, npm test, wrangler deploy).

- [ ] **Step 3: Verify the live site reflects the change**

Using a browser tool, navigate to `https://entheai.com/docs/` and confirm the "Copy page" button is present (it wasn't in the previous deploy), then navigate to `https://entheai.com/llms.txt` and confirm it loads with the expected content.

- [ ] **Step 4: If the workflow fails**

Run `gh run view --repo peterlodri-sec/entheai --log-failed` to see the failing step's output, fix the underlying issue (most likely causes: a markdown frontmatter validation error from Task 2's validation, or a missing/incorrect `CLOUDFLARE_API_TOKEN` permission from Task 12), commit the fix, and push again.
