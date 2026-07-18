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
