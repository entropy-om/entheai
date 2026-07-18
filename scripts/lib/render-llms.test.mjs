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

test("renderLlmsTxt does not truncate at periods inside inline code spans", () => {
  const pages = [
    { id: "config", title: "Configuration", navTitle: "Configuration", group: "Overview", order: 1, body: "Read `entheai.toml` for config. More text here." },
  ];
  const out = renderLlmsTxt(pages, ["Overview"], {
    siteTitle: "test",
    summary: "test",
    baseUrl: "https://test.com",
  });
  // Should contain the full first sentence with code content preserved
  assert.match(out, /Read entheai\.toml for config\./);
});

test("renderLlmsTxt preserves inline code text for grammar", () => {
  const pages = [
    { id: "roles", title: "Roles", navTitle: "Roles", group: "Overview", order: 1, body: "Roles include `coder`, `docs`, and `test`." },
  ];
  const out = renderLlmsTxt(pages, ["Overview"], {
    siteTitle: "test",
    summary: "test",
    baseUrl: "https://test.com",
  });
  // Should preserve inline code text for proper grammar
  assert.match(out, /Roles include coder, docs, and test\./);
});
