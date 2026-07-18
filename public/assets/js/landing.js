/* Landing page interactions: fan-out reveal, terminal demo transcript replay,
   "built on" hover-tint, copy-to-clipboard. */
(function () {
  /* fan-out diagram: reveal children with their own stagger once in view */
  const fanout = document.querySelector("[data-fanout]");
  if (fanout) {
    const io = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (!entry.isIntersecting) return;
          fanout.classList.add("is-seen");
          io.disconnect();
        });
      },
      { threshold: 0.2 }
    );
    io.observe(fanout);
  }

  /* terminal demo: reveal transcript lines on the schedule baked into the markup */
  const demo = document.querySelector("[data-demo]");
  if (demo) {
    const lines = Array.from(demo.querySelectorAll("[data-line]"));
    const cursor = demo.querySelector("[data-demo-cursor]");
    const io = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (!entry.isIntersecting) return;
          io.disconnect();
          if (cursor) cursor.hidden = true;
          lines.forEach((line) => {
            const delay = Number(line.dataset.line) || 0;
            setTimeout(() => {
              line.hidden = false;
              if (cursor && line === lines[lines.length - 1]) {
                setTimeout(() => (cursor.hidden = false), 250);
              }
            }, delay);
          });
        });
      },
      { threshold: 0.2 }
    );
    io.observe(demo);
  }

  /* copy-to-clipboard buttons */
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
})();
