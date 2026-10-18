// Replays the runs recorded by crest on the same board geometry the window
// draws. Nothing is simulated here: packets are interpolated along their
// trace between the recorded send and arrival times, and every chip shows
// the last recorded state at or before the current time.
(function () {
  "use strict";
  const DATA = JSON.parse(document.getElementById("data").textContent);
  const SPEEDS = [0.1, 0.25, 0.5, 1, 2, 4];
  const PLAY_SECONDS = 10;
  const $ = (id) => document.getElementById(id);
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const ms = (us) => (us >= 1e7 ? (us / 1e6).toFixed(2) + " s" : (us / 1000).toFixed(3) + " ms");

  $("title").textContent = DATA.title;
  if (typeof THREE === "undefined" || !window.WebGLRenderingContext) {
    $("stage").innerHTML = '<div id="nogl">WebGL is not available; the checks are listed on the right.</div>';
  }

  const state = { scenario: 0, t: 0, playing: false, speed: 3, selected: null, last: null };
  const scene3 = typeof THREE !== "undefined" ? build() : null;

  function color(c) {
    return new THREE.Color(c[0], c[1], c[2]);
  }

  function build() {
    const canvas = $("board");
    const renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
    renderer.setPixelRatio(window.devicePixelRatio || 1);
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.setClearColor(new THREE.Color(0.010, 0.013, 0.020));
    const scene = new THREE.Scene();
    scene.add(new THREE.HemisphereLight(new THREE.Color(0.2, 0.25, 0.32), new THREE.Color(0.04, 0.05, 0.06), 1.4));
    const sun = new THREE.DirectionalLight(0xfff4e6, 1.6);
    sun.position.set(0.45, 1, 0.65);
    scene.add(sun);

    const S = DATA.scene;
    const b = S.board;
    const center = new THREE.Vector3((b.min[0] + b.max[0]) / 2, 0, (b.min[2] + b.max[2]) / 2);
    const radius = Math.max(1, new THREE.Vector3(b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]).length() / 2);
    const edgeMat = new THREE.LineBasicMaterial({ color: new THREE.Color(0.35, 0.75, 1.0), transparent: true, opacity: 0.55 });
    const box = (min, max, mat, edges) => {
      const g = new THREE.BoxGeometry(max[0] - min[0], max[1] - min[1], max[2] - min[2]);
      const m = new THREE.Mesh(g, mat);
      m.position.set((min[0] + max[0]) / 2, (min[1] + max[1]) / 2, (min[2] + max[2]) / 2);
      scene.add(m);
      if (edges) {
        const l = new THREE.LineSegments(new THREE.EdgesGeometry(g), edgeMat);
        l.position.copy(m.position);
        scene.add(l);
      }
      return m;
    };
    const std = (m) => new THREE.MeshStandardMaterial({ color: color(m.base), emissive: color(m.emissive), metalness: m.metallic, roughness: m.roughness });
    box(b.min, b.max, std(b.material), true);

    for (const r of S.ribbons) {
      const mat = std(r.material);
      for (let i = 0; i + 1 < r.points.length; i++) {
        const a = r.points[i], c = r.points[i + 1];
        const w = r.width / 2;
        const min = [Math.min(a[0], c[0]) - w, 0.0, Math.min(a[2], c[2]) - w];
        const max = [Math.max(a[0], c[0]) + w, a[1] + 0.02, Math.max(a[2], c[2]) + w];
        box(min, max, mat, false);
      }
    }

    const chips = S.chips.map((c) => {
      const min = [c.center[0] - c.size[0] / 2, c.center[1] - c.size[1] / 2, c.center[2] - c.size[2] / 2];
      const max = [c.center[0] + c.size[0] / 2, c.center[1] + c.size[1] / 2, c.center[2] + c.size[2] / 2];
      const body = box(min, max, std(c.material), true);
      const top = new THREE.Mesh(
        new THREE.PlaneGeometry(c.size[0] * 0.92, c.size[2] * 0.92),
        new THREE.MeshStandardMaterial({ color: color(c.material.base), emissive: new THREE.Color(0, 0, 0), roughness: 0.5 })
      );
      top.rotation.x = -Math.PI / 2;
      top.position.set(c.center[0], max[1] + 0.005, c.center[2]);
      scene.add(top);
      for (const p of c.pins) {
        box([p.at[0] - 0.18, p.at[1] - 0.06, p.at[2] - 0.11], [p.at[0] + 0.18, p.at[1] + 0.06, p.at[2] + 0.11],
          new THREE.MeshStandardMaterial({ color: new THREE.Color(0.8, 0.62, 0.25), metalness: 0.9, roughness: 0.35 }), false);
      }
      const ring = new THREE.Mesh(
        new THREE.RingGeometry(0, 1, 4, 1),
        new THREE.MeshBasicMaterial({ color: new THREE.Color(0.5, 0.9, 1.0), transparent: true, opacity: 0.0, side: THREE.DoubleSide })
      );
      ring.rotation.x = -Math.PI / 2;
      ring.rotation.z = Math.PI / 4;
      ring.scale.set(c.size[0] * 0.78, c.size[2] * 0.78, 1);
      ring.position.set(c.center[0], 0.02, c.center[2]);
      scene.add(ring);
      return { def: c, body, top, ring, min, max };
    });

    const packetGeo = new THREE.SphereGeometry(0.28, 12, 8);
    const glowGeo = new THREE.SphereGeometry(0.55, 12, 8);
    const pool = [];
    const packet = (i) => {
      while (pool.length <= i) {
        const core = new THREE.Mesh(packetGeo, new THREE.MeshBasicMaterial());
        const glow = new THREE.Mesh(glowGeo, new THREE.MeshBasicMaterial({ transparent: true, opacity: 0.25, blending: THREE.AdditiveBlending, depthWrite: false }));
        scene.add(core); scene.add(glow);
        pool.push({ core, glow });
      }
      return pool[i];
    };

    const ortho = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.1, radius * 10);
    const persp = new THREE.PerspectiveCamera(34, 1, 0.1, radius * 12);
    const view = { yaw: Math.PI / 4, pitch: Math.PI / 6, zoom: 1, ortho: true, target: center.clone() };
    const iso = () => { view.yaw = Math.PI / 4; view.pitch = Math.PI / 6; view.zoom = 1; view.ortho = true; view.target.copy(center); };

    function camera() {
      const w = canvas.clientWidth || 1, h = canvas.clientHeight || 1, aspect = w / h;
      const dir = new THREE.Vector3(Math.cos(view.pitch) * Math.sin(view.yaw), Math.sin(view.pitch), Math.cos(view.pitch) * Math.cos(view.yaw));
      let cam;
      if (view.ortho) {
        const vh = (radius * 1.25) / view.zoom;
        ortho.left = (-vh * aspect) / 2; ortho.right = (vh * aspect) / 2; ortho.top = vh / 2; ortho.bottom = -vh / 2;
        ortho.position.copy(view.target).addScaledVector(dir, radius * 4);
        cam = ortho;
      } else {
        persp.aspect = aspect;
        persp.position.copy(view.target).addScaledVector(dir, (radius * 2.6) / view.zoom);
        cam = persp;
      }
      cam.lookAt(view.target);
      cam.updateProjectionMatrix();
      return cam;
    }

    function resize() {
      const w = canvas.clientWidth, h = canvas.clientHeight;
      if (canvas.width !== Math.floor(w * renderer.getPixelRatio()) || canvas.height !== Math.floor(h * renderer.getPixelRatio())) {
        renderer.setSize(w, h, false);
      }
    }

    function draw(frame) {
      resize();
      for (const [i, c] of chips.entries()) {
        const look = frame.looks[i];
        const glow = look ? look.emissive : [0.2, 0.2, 0.2];
        const k = look && !look.up ? 1.2 : look && look.busy ? 0.8 : 0.12;
        c.top.material.emissive = color(glow).multiplyScalar(k);
        c.ring.material.opacity = state.selected === i ? 0.35 : 0.0;
      }
      let n = 0;
      for (const p of frame.packets) {
        const parts = [{ at: p.at, s: 1 }].concat(p.trail.map((t, j) => ({ at: t, s: 0.8 - j * 0.15 })));
        for (const part of parts) {
          const m = packet(n++);
          m.core.visible = m.glow.visible = true;
          m.core.position.set(part.at[0], part.at[1], part.at[2]);
          m.glow.position.copy(m.core.position);
          m.core.scale.setScalar(part.s);
          m.glow.scale.setScalar(part.s);
          m.core.material.color = color(p.color).multiplyScalar(1.6);
          m.glow.material.color = color(p.color);
        }
      }
      for (let i = n; i < pool.length; i++) pool[i].core.visible = pool[i].glow.visible = false;
      const cam = camera();
      renderer.render(scene, cam);
      const labels = $("labels");
      const w = canvas.clientWidth, h = canvas.clientHeight;
      labels.innerHTML = S.labels.map((l) => {
        const v = new THREE.Vector3(l.at[0], l.at[1], l.at[2]).project(cam);
        if (v.z < -1 || v.z > 1) return "";
        const i = S.chips.findIndex((c) => c.id === l.chip);
        const look = frame.looks[i];
        const g = look ? look.emissive.map((x) => Math.round(Math.sqrt(Math.min(1, x)) * 255)) : [140, 150, 160];
        return `<div class="label" style="left:${((v.x + 1) / 2) * w}px;top:${((1 - v.y) / 2) * h - 30}px"><b>${esc(l.text)}</b><span style="color:rgb(${g.join(",")})">${esc(look ? look.state : "")}</span></div>`;
      }).join("");
    }

    function pick(x, y) {
      const cam = camera();
      const ray = new THREE.Raycaster();
      ray.setFromCamera(new THREE.Vector2((x / canvas.clientWidth) * 2 - 1, -(y / canvas.clientHeight) * 2 + 1), cam);
      let best = null, dist = Infinity;
      chips.forEach((c, i) => {
        const hit = ray.ray.intersectBox(new THREE.Box3(new THREE.Vector3(...c.min), new THREE.Vector3(...c.max)), new THREE.Vector3());
        if (hit) { const d = hit.distanceTo(ray.ray.origin); if (d < dist) { dist = d; best = i; } }
      });
      return best;
    }

    let drag = null;
    canvas.addEventListener("contextmenu", (e) => e.preventDefault());
    canvas.addEventListener("pointerdown", (e) => { drag = { x: e.clientX, y: e.clientY, sx: e.clientX, sy: e.clientY, pan: e.button !== 0 || e.shiftKey }; canvas.setPointerCapture(e.pointerId); });
    canvas.addEventListener("pointermove", (e) => {
      if (!drag) return;
      const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
      drag.x = e.clientX; drag.y = e.clientY;
      if (drag.pan) {
        const vh = view.ortho ? (radius * 1.25) / view.zoom : 2 * ((radius * 2.6) / view.zoom) * Math.tan(0.3);
        const per = vh / (canvas.clientHeight || 1);
        const right = new THREE.Vector3(Math.cos(view.yaw), 0, -Math.sin(view.yaw));
        const fwd = new THREE.Vector3(-Math.sin(view.yaw), 0, -Math.cos(view.yaw));
        view.target.addScaledVector(right, -dx * per).addScaledVector(fwd, dy * per);
      } else {
        view.ortho = false;
        view.yaw -= dx * 0.008;
        view.pitch = Math.min(1.48, Math.max(0.08, view.pitch + dy * 0.008));
      }
      redraw();
    });
    canvas.addEventListener("pointerup", (e) => {
      if (drag && Math.abs(e.clientX - drag.sx) < 4 && Math.abs(e.clientY - drag.sy) < 4) {
        const r = canvas.getBoundingClientRect();
        state.selected = pick(e.clientX - r.left, e.clientY - r.top);
        panel();
        redraw();
      }
      drag = null;
    });
    canvas.addEventListener("wheel", (e) => { e.preventDefault(); view.zoom = Math.min(8, Math.max(0.25, view.zoom * Math.pow(1.15, -e.deltaY / 100))); redraw(); }, { passive: false });
    $("iso").onclick = () => { iso(); redraw(); };
    $("persp").onclick = () => { view.ortho = false; redraw(); };
    window.addEventListener("resize", () => redraw());
    return { draw, iso };
  }

  function run() { return DATA.scenarios[state.scenario]; }

  function along(points, f) {
    let total = 0;
    const seg = [];
    for (let i = 0; i + 1 < points.length; i++) {
      const a = points[i], b = points[i + 1];
      const l = Math.hypot(b[0] - a[0], b[2] - a[2]);
      seg.push(l); total += l;
    }
    let left = Math.min(1, Math.max(0, f)) * total;
    for (let i = 0; i < seg.length; i++) {
      if (left <= seg[i] && seg[i] > 0) {
        const t = left / seg[i], a = points[i], b = points[i + 1];
        return [a[0] + (b[0] - a[0]) * t, DATA.packet_y, a[2] + (b[2] - a[2]) * t];
      }
      left -= seg[i];
    }
    const last = points[points.length - 1];
    return [last[0], DATA.packet_y, last[2]];
  }

  // Same rule as the window: a packet is in flight while sent <= t < arrival.
  function frame(t) {
    const r = run();
    const packets = [];
    for (const [ribbon, at, arrive, kind, c] of r.trips) {
      if (at > t || arrive <= t || arrive <= at) continue;
      const pts = DATA.scene.ribbons[ribbon].points;
      const f = (t - at) / (arrive - at);
      packets.push({ kind, color: c, at: along(pts, f), trail: [1, 2, 3, 4].map((k) => along(pts, Math.max(0, f - 0.05 * k))) });
    }
    const looks = DATA.scene.chips.map(() => null);
    for (const k of r.keys) {
      if (k[0] > t) break;
      looks[k[1]] = { state: k[2], emissive: k[3], busy: k[4], up: k[5], queued: k[6], vars: k[7], queue: k[8] };
    }
    return { packets, looks };
  }

  function panel() {
    const r = run();
    const f = frame(state.t);
    const insp = $("inspector");
    if (state.selected === null || state.selected === undefined) {
      insp.className = "muted";
      insp.textContent = "Click a chip to inspect it.";
    } else {
      const c = DATA.scene.chips[state.selected];
      const l = f.looks[state.selected];
      const vars = l ? Object.entries(l.vars).map(([k, v]) => `<div class="mono">${esc(k)} = ${v}</div>`).join("") : "";
      const queue = l && l.queue.length ? `<div class="muted">queue: ${l.queue.map(esc).join(", ")}</div>` : "";
      const status = l ? (l.up ? esc(l.state) + (l.busy ? " · busy" : "") + (l.queued ? ` · ${l.queued} queued` : "") : esc(l.state) + " · down") : "";
      insp.className = "";
      insp.innerHTML = `<h2>${esc(c.label)} · ${esc(c.kind)}</h2><div>${status}</div>${vars}${queue}`;
    }
    $("checks").innerHTML = r.verdicts.map((v, i) => `<div class="check${v.at_us !== null ? " link" : ""}" data-i="${i}"><div class="p ${v.passed ? "ok" : "bad"}">${v.passed ? "✓" : "✗"} ${esc(v.property)}</div><div class="d">${esc(v.detail)}</div></div>`).join("");
    for (const el of document.querySelectorAll(".check.link")) {
      el.onclick = () => { const v = r.verdicts[+el.dataset.i]; state.playing = false; state.t = Math.min(r.until_us, v.at_us); redraw(); };
    }
  }

  function tabs() {
    $("tabs").innerHTML = DATA.scenarios.map((s, i) => `<button data-i="${i}" class="${s.passed ? "pass" : "fail"}${i === state.scenario ? " on" : ""}">${s.passed ? "✓" : "✗"} ${esc(s.id)}</button>`).join(" ");
    for (const el of $("tabs").querySelectorAll("button")) {
      el.onclick = () => { state.scenario = +el.dataset.i; state.t = 0; state.playing = false; tabs(); redraw(); };
    }
    const passed = DATA.scenarios.filter((s) => s.passed).length;
    $("summary").textContent = `${passed}/${DATA.scenarios.length} pass · seed ${run().seed}`;
    const r = run();
    $("markers").innerHTML = r.verdicts.filter((v) => !v.passed && v.at_us !== null)
      .map((v) => `<div class="marker" title="${esc(v.property)}" style="left:${(Math.min(r.until_us, v.at_us) / r.until_us) * 100}%"></div>`).join("");
  }

  function redraw() {
    const r = run();
    $("time").textContent = `${ms(state.t)} / ${ms(r.until_us)}`;
    $("range").value = String(Math.round((state.t / r.until_us) * 1000));
    $("play").textContent = state.playing ? "⏸" : "▶";
    $("speed").textContent = SPEEDS[state.speed] + "×";
    if (scene3) scene3.draw(frame(state.t));
    panel();
  }

  function tick(now) {
    if (state.playing) {
      const r = run();
      if (state.last !== null) {
        state.t = Math.min(r.until_us, state.t + ((r.until_us / PLAY_SECONDS) * SPEEDS[state.speed] * (now - state.last)) / 1000);
        if (state.t >= r.until_us) state.playing = false;
      }
      state.last = now;
      redraw();
    } else {
      state.last = null;
    }
    requestAnimationFrame(tick);
  }

  const step = (dir) => {
    const times = run().times;
    state.playing = false;
    if (dir > 0) { const n = times.find((x) => x > state.t); state.t = n === undefined ? run().until_us : n; }
    else { let p = 0; for (const x of times) { if (x < state.t) p = x; else break; } state.t = p; }
    redraw();
  };
  const toggle = () => { if (!state.playing && state.t >= run().until_us) state.t = 0; state.playing = !state.playing; redraw(); };
  $("play").onclick = toggle;
  $("back").onclick = () => step(-1);
  $("fwd").onclick = () => step(1);
  $("slower").onclick = () => { state.speed = Math.max(0, state.speed - 1); redraw(); };
  $("faster").onclick = () => { state.speed = Math.min(SPEEDS.length - 1, state.speed + 1); redraw(); };
  $("range").oninput = (e) => { state.t = (run().until_us * +e.target.value) / 1000; redraw(); };
  window.addEventListener("keydown", (e) => {
    if (e.key === " ") { e.preventDefault(); toggle(); }
    else if (e.key === "ArrowRight") step(1);
    else if (e.key === "ArrowLeft") step(-1);
    else if (e.key === "i" && scene3) { scene3.iso(); redraw(); }
  });
  tabs();
  redraw();
  requestAnimationFrame(tick);
})();
