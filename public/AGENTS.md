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
