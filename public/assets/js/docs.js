/* entheai docs shell: section switching, prev/next, search, theme toggle, TOC. */
(function () {
  const seen = new Set();
  const FLAT = Array.from(document.querySelectorAll("[data-nav-item]"))
    .filter((el) => {
      if (seen.has(el.dataset.navItem)) return false;
      seen.add(el.dataset.navItem);
      return true;
    })
    .map((el) => ({
      id: el.dataset.navItem,
      label: el.textContent.trim(),
      group: el.closest("[data-nav-group]")?.dataset.navGroup || "",
    }));

  const pages = document.querySelectorAll("[data-page]");
  const navButtons = document.querySelectorAll("[data-nav-item]");
  const prevBtn = document.querySelector("[data-nav-prev]");
  const nextBtn = document.querySelector("[data-nav-next]");
  const drawer = document.querySelector("[data-drawer]");
  const burger = document.querySelector("[data-burger]");
  const themeBtn = document.querySelector("[data-theme-toggle]");
  const searchInput = document.querySelector("[data-search-input]");
  const searchResults = document.querySelector("[data-search-results]");
  const toc = document.querySelector("[data-toc]");

  function idx(id) {
    return FLAT.findIndex((f) => f.id === id);
  }

  function buildToc(id) {
    if (!toc) return;
    const page = document.querySelector(`[data-page="${id}"]`);
    const heads = page ? Array.from(page.querySelectorAll("h2[id]")) : [];
    toc.innerHTML = "";
    if (!heads.length) {
      toc.innerHTML = '<div style="color:var(--text-faint)">—</div>';
      return;
    }
    heads.forEach((h) => {
      const a = document.createElement("a");
      a.href = "#" + h.id;
      a.textContent = h.textContent;
      a.style.display = "block";
      a.style.padding = "4px 0";
      a.style.color = "var(--text-muted)";
      toc.appendChild(a);
    });
  }

  function go(id) {
    pages.forEach((p) => (p.hidden = p.dataset.page !== id));
    navButtons.forEach((b) => b.classList.toggle("is-active", b.dataset.navItem === id));
    const i = idx(id);
    const prev = FLAT[i - 1];
    const next = FLAT[i + 1];
    if (prevBtn) {
      if (prev) {
        prevBtn.hidden = false;
        prevBtn.dataset.navItem = prev.id;
        prevBtn.querySelector("[data-label]").textContent = prev.label;
      } else {
        prevBtn.hidden = true;
      }
    }
    if (nextBtn) {
      if (next) {
        nextBtn.hidden = false;
        nextBtn.dataset.navItem = next.id;
        nextBtn.querySelector("[data-label]").textContent = next.label;
      } else {
        nextBtn.hidden = true;
      }
    }
    buildToc(id);
    if (drawer) drawer.hidden = true;
    window.scrollTo(0, 0);
    history.replaceState(null, "", "#" + id);
  }

  document.querySelectorAll("[data-nav-item], [data-nav-prev], [data-nav-next]").forEach((el) => {
    el.addEventListener("click", (e) => {
      e.preventDefault();
      go(el.dataset.navItem);
    });
  });

  if (burger && drawer) {
    burger.addEventListener("click", () => {
      drawer.hidden = !drawer.hidden;
    });
    drawer.addEventListener("click", (e) => {
      if (e.target === drawer) drawer.hidden = true;
    });
  }

  if (themeBtn) {
    const stored = localStorage.getItem("entheai-theme");
    if (stored) document.documentElement.setAttribute("data-theme", stored);
    themeBtn.textContent = document.documentElement.getAttribute("data-theme") === "light" ? "☀" : "☾";
    themeBtn.addEventListener("click", () => {
      const cur = document.documentElement.getAttribute("data-theme") === "light" ? "light" : "dark";
      const next = cur === "light" ? "dark" : "light";
      document.documentElement.setAttribute("data-theme", next);
      localStorage.setItem("entheai-theme", next);
      themeBtn.textContent = next === "light" ? "☀" : "☾";
    });
  }

  if (searchInput && searchResults) {
    searchInput.addEventListener("input", () => {
      const q = searchInput.value.trim().toLowerCase();
      if (!q) {
        searchResults.hidden = true;
        searchResults.innerHTML = "";
        return;
      }
      const matches = FLAT.filter((f) => f.label.toLowerCase().includes(q));
      searchResults.hidden = false;
      if (!matches.length) {
        searchResults.innerHTML = '<div class="search-empty">No matches</div>';
        return;
      }
      searchResults.innerHTML = "";
      matches.forEach((m) => {
        const btn = document.createElement("button");
        btn.className = "search-hit";
        btn.innerHTML = `${m.label} <span class="search-hit-group">${m.group}</span>`;
        btn.addEventListener("click", () => {
          go(m.id);
          searchInput.value = "";
          searchResults.hidden = true;
        });
        searchResults.appendChild(btn);
      });
    });
  }

  document.querySelectorAll("[data-copy]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const text = btn.getAttribute("data-copy");
      navigator.clipboard.writeText(text).catch(() => {});
      const label = btn.textContent;
      btn.textContent = "copied ✓";
      btn.classList.add("is-copied");
      setTimeout(() => {
        btn.textContent = label;
        btn.classList.remove("is-copied");
      }, 1400);
    });
  });

  const initial = (location.hash || "#what-is").slice(1);
  go(FLAT.some((f) => f.id === initial) ? initial : "what-is");
})();
