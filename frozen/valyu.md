+++
name = "valyu"
domain = "deep research with citations"
triggers = ["research", "deep research", "cite", "citation", "sources", "valyu", "literature", "state of the art", "papers", "academic", "find evidence"]
mcp = "valyu"
rank = 1.0
+++
When a task needs **grounded, cited answers** — best-practices, prior art, "what's the
current state of the art", literature, market/company facts — use the **Valyu** MCP rather
than guessing from memory. It searches the live web + proprietary corpora (arXiv, PubMed,
SEC, FRED, …) and returns sources with relevance scores.

**Tools:** `valyu_search` (web/general), `valyu_academic_search` (arXiv/PubMed/bioRxiv),
`valyu_financial_search` / `valyu_economics_search` / `valyu_company_research` for
markets, `valyu_bio_search` for biomed. One clear natural-language question per topic —
the API rewrites + reranks; don't keyword-stuff.

**Discipline:** treat results as *sources to verify*, not truth — cross-check across
results, prefer primary/authoritative ones, and quote/cite. Big result sets can blow the
context budget: extract just the needed spans, don't read whole dumps in. Deep-research a
claim before committing to a pattern you're unsure of.
