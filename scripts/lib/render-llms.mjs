function firstSentence(markdownBody) {
  const plain = markdownBody
    .replace(/```[\s\S]*?```/g, "")
    .replace(/^>.*$/gm, "")
    .replace(/`([^`]*)`/g, (_m, inner) => inner.replace(/\./g, "@@DOT@@"))
    .replace(/[#*_]/g, "")
    .replace(/\s+/g, " ")
    .trim();
  const match = plain.match(/^[^.!?]*[.!?]/);
  const result = (match ? match[0] : plain.slice(0, 140)).trim();
  return result.replace(/@@DOT@@/g, ".");
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
