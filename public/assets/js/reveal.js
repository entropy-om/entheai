/* Reveal-on-scroll for elements with .reveal, plus opt-in staggered delay
   via data-delay (ms). Uses IntersectionObserver, disconnects once seen. */
(function () {
  const els = document.querySelectorAll(".reveal");
  const io = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) return;
        const el = entry.target;
        const delay = Number(el.dataset.delay) || 0;
        setTimeout(() => el.classList.add("is-seen"), delay);
        io.unobserve(el);
      });
    },
    { threshold: 0.2 }
  );
  els.forEach((el) => io.observe(el));
})();
