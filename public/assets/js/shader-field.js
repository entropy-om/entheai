/* entheai ShaderField — GPU-cheap animated canvas backdrop.
   A drifting field of bioluminescent blooms with N-fold radial symmetry,
   low-contrast and slow. Honors prefers-reduced-motion. */
(function () {
  function mount(wrap) {
    const canvas = wrap.querySelector("canvas");
    const ctx = canvas.getContext("2d");
    const symmetry = Number(wrap.dataset.symmetry) || 6;
    const density = Number(wrap.dataset.density) || 0.9;
    const speed = Number(wrap.dataset.speed) || 1;
    const intensity = Number(wrap.dataset.intensity) || 0.9;
    const interactive = wrap.dataset.interactive !== "false";

    let raf, w, h, dpr, running = true, t = 0;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const pointer = { x: 0.5, y: 0.5, tx: 0.5, ty: 0.5 };

    const teal = [47, 240, 226], cyan = [56, 200, 255], mag = [255, 63, 180];
    const N = Math.max(3, Math.round(7 * density));
    const blobs = Array.from({ length: N }, (_, i) => ({
      a: Math.random() * Math.PI * 2,
      r: 0.12 + Math.random() * 0.34,
      sp: (0.06 + Math.random() * 0.14) * (Math.random() < 0.5 ? 1 : -1),
      rad: 0.16 + Math.random() * 0.26,
      col: i % 3 === 0 ? mag : i % 2 === 0 ? cyan : teal,
      ph: Math.random() * 6.28,
    }));

    function resize() {
      dpr = Math.min(window.devicePixelRatio || 1, 1.75);
      const b = wrap.getBoundingClientRect();
      w = Math.max(1, b.width);
      h = Math.max(1, b.height);
      canvas.width = w * dpr;
      canvas.height = h * dpr;
      canvas.style.width = w + "px";
      canvas.style.height = h + "px";
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    }

    function draw() {
      if (!running) return;
      t += 0.0016 * speed;
      ctx.globalCompositeOperation = "source-over";
      ctx.fillStyle = "rgba(4,6,10,0.32)";
      ctx.fillRect(0, 0, w, h);
      const cx = w / 2, cy = h / 2;
      const R = Math.hypot(w, h) * 0.5;
      pointer.x += (pointer.tx - pointer.x) * 0.05;
      pointer.y += (pointer.ty - pointer.y) * 0.05;
      const warpX = interactive ? (pointer.x - 0.5) * 0.18 : 0;
      const warpY = interactive ? (pointer.y - 0.5) * 0.18 : 0;
      ctx.globalCompositeOperation = "lighter";
      const slice = (Math.PI * 2) / symmetry;
      for (const b of blobs) {
        b.a += b.sp * 0.004 * speed;
        const br = (b.r + 0.05 * Math.sin(t * 1.3 + b.ph) + warpX) * R;
        const bx = Math.cos(b.a) * br;
        const by = Math.sin(b.a) * br * 0.8 + warpY * R;
        const rad = b.rad * R * (0.85 + 0.25 * Math.sin(t * 0.9 + b.ph));
        const [cr, cg, cb] = b.col;
        for (let s = 0; s < symmetry; s++) {
          const ang = s * slice + t * 0.05;
          const px = cx + bx * Math.cos(ang) - by * Math.sin(ang);
          const py = cy + bx * Math.sin(ang) + by * Math.cos(ang);
          const g = ctx.createRadialGradient(px, py, 0, px, py, rad);
          const alpha = 0.16 * intensity;
          g.addColorStop(0, `rgba(${cr},${cg},${cb},${alpha})`);
          g.addColorStop(0.4, `rgba(${cr},${cg},${cb},${alpha * 0.4})`);
          g.addColorStop(1, "rgba(0,0,0,0)");
          ctx.fillStyle = g;
          ctx.beginPath();
          ctx.arc(px, py, rad, 0, Math.PI * 2);
          ctx.fill();
        }
      }
      const core = ctx.createRadialGradient(cx + warpX * R, cy, 0, cx + warpX * R, cy, R * 0.22);
      core.addColorStop(0, `rgba(110,247,236,${0.12 * intensity})`);
      core.addColorStop(1, "rgba(0,0,0,0)");
      ctx.fillStyle = core;
      ctx.beginPath();
      ctx.arc(cx + warpX * R, cy, R * 0.22, 0, Math.PI * 2);
      ctx.fill();
      raf = requestAnimationFrame(draw);
    }

    resize();
    window.addEventListener("resize", resize);
    function onMove(e) {
      const b = wrap.getBoundingClientRect();
      pointer.tx = (e.clientX - b.left) / b.width;
      pointer.ty = (e.clientY - b.top) / b.height;
    }
    if (interactive && !reduce) window.addEventListener("pointermove", onMove);
    if (reduce) {
      ctx.fillStyle = "rgba(4,6,10,1)";
      ctx.fillRect(0, 0, w, h);
    } else {
      draw();
    }
  }

  document.querySelectorAll("[data-shader-field]").forEach(mount);
})();
