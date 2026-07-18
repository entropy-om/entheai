# claude-design prompts — entheai

Three ready-to-paste prompts for generating the entheai web presence. Each is self-contained; paste one at a time. Shared brand direction is repeated in each so they can be used independently.

**Brand at a glance (used by all three):**
`entheai` is a personal, macOS/Apple-Silicon, terminal-native **hybrid coding agent** written in Rust. A cloud orchestrator (DeepSeek V4 Pro) plans; it **fans out** to a swarm of model-matched sub-agents that run in parallel git worktrees and merge back verified. It runs local models via Osaurus, has a **visual TUI** (shader backgrounds + a live 3D codebase graph), compounding memory, self-learning, and personalization. Lineage/motifs: deep-sea + bioluminescence (the "whale" ecosystem — CodeWhale, ultrawhale), a touch of dinosaur (Osaurus), glamorous-terminal cyberpunk. **Aesthetic:** dark-first, near-black backdrops, bioluminescent teal/cyan + electric magenta accents, subtle animated shader-like gradient/noise background, crisp monospace (JetBrains Mono / Berkeley Mono vibe) for code and UI chrome, generous whitespace, high contrast, tasteful motion. Theme-aware (support light too, but dark is the hero). Fully self-contained, responsive, accessible (WCAG AA), no external asset dependencies.

---

## 1 — Landing page

> Design and build a single-page marketing landing page for **entheai**, a personal macOS/Apple-Silicon hybrid coding agent for the terminal (Rust). 
>
> **Vibe:** "glamorous terminal meets deep-sea bioluminescence." Dark, near-black background with a subtle animated shader-like gradient/noise field (slow, low-contrast, GPU-cheap — think a Cyberpunk/RandomShader backdrop behind the content, not distracting). Bioluminescent teal/cyan + electric magenta accents; crisp monospace for headings/code; smooth scroll-reveal motion.
>
> **Sections, in order:**
> 1. **Hero** — product name `entheai`, tagline *"A coding agent with a brain that fans out."*, one-line subhead ("Cloud plans. A swarm of model-matched sub-agents builds — in parallel, in isolated worktrees, merged only after tests pass."), a primary CTA button ("Get started") and a secondary ("View on GitHub" → https://github.com/peterlodri-sec/entheai). Behind it, the animated shader field. Include a small "macOS · Apple Silicon" badge.
> 2. **The idea** — a clean diagram/illustration of the fan-out: an orchestrator node branching into parallel coder/docs/test/review sub-agents (each tagged with a different model), converging into a "merge + verify" node. Animate the fan-out on scroll.
> 3. **Feature grid** (6 cards, icon + title + one sentence): Tiered hybrid brain · Fan-out orchestration · Visual TUI (shader backgrounds + live codebase graph) · Compounding local memory + auto-compaction · Extensible (skills · plugins · MCP) · Self-improving & personalized.
> 4. **"Shows, don't tells"** — a stylized terminal window mockup with a syntax-highlighted transcript: a user prompt, the agent planning, fanning out to N coders on different models, running tests, and integrating. Blinking cursor, window chrome, monospace.
> 5. **Built on** — a tidy logo/wordmark row: Osaurus, CodeWhale, Crush, Ruflo, OpenCode Zen, Honcho, Tailscale (text wordmarks are fine).
> 6. **Footer** — repo link, "made for one Mac, by one developer," license note (TBD).
>
> Keep it to one page, fast, no bloat, all assets inline (CSS/JS/SVG). Dark-first, theme-aware. Make the motion elegant and restrained.

---

## 2 — Documentation site

> Design and build a **documentation site** for **entheai** (a macOS/Apple-Silicon hybrid coding-agent CLI in Rust). Match the landing-page brand: dark-first, near-black, bioluminescent teal/cyan + magenta accents, monospace for code and nav chrome, subtle shader-like backdrop kept very low-key so it never competes with text.
>
> **Layout:** classic docs shell — a left sidebar with collapsible sections, a sticky top bar with the `entheai` wordmark + a search box + a light/dark toggle, a main content column with a right-hand "on this page" table of contents, and prev/next links at the bottom of each page. Fully responsive (sidebar collapses to a drawer on mobile).
>
> **Information architecture (sidebar):**
> - **Overview** — what entheai is, the hybrid-brain / fan-out concept, who it's for.
> - **Getting started** — install (git clone + `cargo build --release`), configure `entheai.toml`, first run.
> - **Configuration** — providers (Osaurus local, OpenCode Zen, DeepSeek, OpenRouter), models, the `<provider>/<model>` id convention.
> - **Concepts** — the agent loop, the tiered router, fan-out & sub-agent roles, the permission gate & YOLO mode, memory (the five namespaces), skills, plugins, MCP.
> - **The visual TUI** — shader backgrounds, the codebase graph toggle, keybindings.
> - **Architecture** — the Rust crate map + a system diagram (orchestrator, providers, Osaurus, codebase-memory MCP, sub-agents).
> - **Roadmap & design docs** — link out to the spec/plans.
>
> **Content styling:** first-class code blocks (copy button, monospace, syntax highlighting, teal/magenta tokens on dark), callout/admonition boxes (note / warning / tip) in the accent palette, tables for config keys, and inline `code` chips. Populate each page with realistic placeholder content drawn from the IA above (short, accurate, skimmable). Prioritize legibility and fast navigation. All self-contained, accessible, theme-aware.

---

## 3 — Usage guide frontend (interactive getting-started)

> Design and build an **interactive usage guide** (a single scrollytelling / step-by-step page) that teaches a new user how to drive **entheai** — a macOS/Apple-Silicon hybrid coding agent for the terminal. Same brand as the landing page: dark, bioluminescent-terminal cyberpunk, monospace, restrained shader-like backdrop, elegant motion.
>
> **Format:** a guided, numbered walkthrough where each step pairs a short explanation (left) with an **interactive terminal mockup** (right) that "types out" the relevant command and a simulated response when the step scrolls into view. Include a persistent progress indicator (step N of M) and a floating "copy all commands" button. Each command block has its own copy button.
>
> **Steps:**
> 1. **Install & build** — `git clone` + `cargo build --release`; show the build finishing.
> 2. **Point it at a model** — create `entheai.toml` (show both a local Osaurus config and a cloud OpenCode Zen config in a tabbed code block).
> 3. **First conversation** — `entheai "summarize this repo"`; show tokens streaming in the mock terminal.
> 4. **Let it use tools** — `entheai "add a --version flag and run the tests"`; simulate the agent calling `read_file` → `write_file` → `run_shell`, with a **permission prompt** (`allow run_shell(...)? [y/N]`) the user can "click" y/N in the mockup.
> 5. **YOLO mode** — `entheai --yolo "..."`; explain auto-approval and show it skipping prompts.
> 6. **Fan-out on a big task** — `entheai "implement feature X across the codebase"`; visualize the orchestrator spawning several model-matched coders in parallel worktrees, then merging + testing (a small animated fan-out).
> 7. **Skills & the codebase graph** — invoke a bundled skill; toggle the live 3D codebase graph.
>
> End with a "You're set" panel linking to the full docs and the GitHub repo (https://github.com/peterlodri-sec/entheai). Make the terminal simulations feel real (blinking cursor, realistic timing, syntax highlighting) but keep everything self-contained, responsive, accessible, and theme-aware. The goal: someone finishes this page and immediately knows how to be productive in entheai.
