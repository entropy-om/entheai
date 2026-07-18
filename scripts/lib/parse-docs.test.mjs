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

test("throws on non-numeric order", () => {
  const dir = makeFixtureDir({
    "a.md": `---\nid: a\ntitle: A\ngroup: Overview\norder: abc\n---\nBody\n`,
  });
  assert.throws(() => loadPages(dir), /"order" must be a number/);
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
