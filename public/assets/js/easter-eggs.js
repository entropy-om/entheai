/* Hidden bits: a console hello for curious devs, and a Konami-code unlock. */
(function () {
  try {
    console.log(
      "%c entheai %c fans out, one Mac at a time.\nCurious? Try the Konami code.",
      "background:#04060a;color:#2ff0e2;font-family:monospace;font-weight:700;padding:4px 8px;border-radius:4px 0 0 4px;",
      "background:#04060a;color:#b3c4cd;font-family:monospace;padding:4px 8px;border-radius:0 4px 4px 0;"
    );
  } catch (_) {}

  const SEQUENCE = ["ArrowUp", "ArrowUp", "ArrowDown", "ArrowDown", "ArrowLeft", "ArrowRight", "ArrowLeft", "ArrowRight", "b", "a"];
  let pos = 0;
  window.addEventListener("keydown", (e) => {
    const matches = e.key === SEQUENCE[pos];
    pos = matches ? pos + 1 : e.key === SEQUENCE[0] ? 1 : 0;
    if (pos === SEQUENCE.length) {
      pos = 0;
      unlock();
    }
  });

  function unlock() {
    if (document.querySelector(".egg-toast")) return;

    if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      const burst = document.createElement("div");
      burst.className = "egg-burst";
      document.body.appendChild(burst);
      setTimeout(() => burst.remove(), 1300);
    }

    const toast = document.createElement("a");
    toast.className = "egg-toast";
    toast.href = "/vaked";
    toast.innerHTML = '<span class="egg-toast-prompt">›</span> secret unlocked — <span class="egg-toast-link">/vaked</span>';
    document.body.appendChild(toast);
    requestAnimationFrame(() => toast.classList.add("is-seen"));
    setTimeout(() => {
      toast.classList.remove("is-seen");
      setTimeout(() => toast.remove(), 400);
    }, 5000);
  }
})();
