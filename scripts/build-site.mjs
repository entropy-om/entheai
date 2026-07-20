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

// The Vaked Genesis Seal — SHA-256 of the genesis archive (origin:
// peterlodri-sec/vaked-base, genesis_snapshot.md). Sourced from the GENESIS_HASH
// build-arg (env) so the deploy can inject/rotate it; falls back to the sealed
// default committed here.
const DEFAULT_GENESIS_HASH =
  "7c242080f5f821e5eaf563fe2208d60632c451687baf65f4fe8e4a0d226e3ecf";

/** Resolve the genesis hash from the `GENESIS_HASH` build-arg, validated. */
export function genesisHash(env = process.env) {
  const h = (env.GENESIS_HASH ?? DEFAULT_GENESIS_HASH).trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(h)) {
    throw new Error(`GENESIS_HASH must be 64 hex chars; got ${JSON.stringify(h)}`);
  }
  return h;
}

export function build({
  root,
  baseUrl = DEFAULT_BASE_URL,
  siteTitle = DEFAULT_SITE_TITLE,
  siteSummary = DEFAULT_SITE_SUMMARY,
  genesis = genesisHash(),
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

  // Inject the Vaked Genesis Seal into the landing footer from the build-arg.
  // Idempotent — replaces the `<code data-genesis-seal>` content each build.
  // Skipped when public/index.html is absent (e.g. the test fixture).
  const indexPath = path.join(publicDir, "index.html");
  let genesisInjected = false;
  if (fs.existsSync(indexPath)) {
    const html = fs.readFileSync(indexPath, "utf8");
    const re = /(<code data-genesis-seal>)[0-9a-f]*(<\/code>)/;
    if (re.test(html)) {
      fs.writeFileSync(indexPath, html.replace(re, `$1${genesis}$2`));
      genesisInjected = true;
    }
  }

  return {
    pageCount: pages.length,
    outputDocsHtml,
    outputLlmsTxt,
    outputLlmsFullTxt,
    genesis,
    genesisInjected,
  };
}

function main() {
  const __dirname = path.dirname(fileURLToPath(import.meta.url));
  const root = path.join(__dirname, "..");
  const result = build({ root });
  console.log(`Built ${result.pageCount} docs pages -> ${result.outputDocsHtml}`);
  console.log(`Wrote ${result.outputLlmsTxt}`);
  console.log(`Wrote ${result.outputLlmsFullTxt}`);
  console.log(
    `Genesis seal ${result.genesisInjected ? "injected into" : "not found in"} the footer: ${result.genesis}`
  );
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main();
}
