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
