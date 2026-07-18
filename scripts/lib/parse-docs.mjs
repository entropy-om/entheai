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
    const order = Number(data.order);
    if (!Number.isFinite(order)) {
      throw new Error(`${file}: "order" must be a number, got "${data.order}"`);
    }

    return {
      id: String(data.id),
      title: String(data.title),
      navTitle: data.navTitle ? String(data.navTitle) : String(data.title),
      group: String(data.group),
      order,
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
