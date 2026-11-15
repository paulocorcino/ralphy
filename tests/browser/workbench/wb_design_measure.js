// The page side of wb_design_measure.py: injected with `evaluate` (the page CSP
// refuses an inline script), it defines `window.DSP`.
window.DSP = (() => {
  const C = {
    "btn": ".btn:not(.accent):not(.danger)", "btn-accent": ".btn.accent", "btn-danger": ".btn.danger",
    "modal": ".modal", "modal-head": ".modal-head", "modal-title": ".modal-title",
    "modal-foot": ".modal-foot", "modal-x": ".modal-x", "foot-note": ".foot-note",
    "dropdown": ".dropdown", "dropdown-head": ".dropdown-head",
    "menu-row": ".dropdown-item,.window-item,.fence-item,.note-item,.column-item,.console-tab-item",
    "ctx-item": ".ctx-item", "set-input": ".set-input", "host-input": ".host-input",
    "set-label": ".set-label", "set-row": ".set-row", "seg": ".seg", "seg-btn": ".seg-btn",
    "seg-files": ".files-search .seg button", "tab": ".tab", "tab-close": ".tab-close",
    "checkbox": "input[type=checkbox]:not(.switch input)", "switch": ".switch",
    "card": ".sec-card", "card-title": ".sec-card-title", "kbd": ".kbd", "toast": ".wb-toast",
    "rel-cmd": ".rel-cmd", "badge": "[class*='-badge']", "chip": "[class*='-chip']",
    "empty": "[class$='-empty'],[class*='-empty ']", "error": "[class$='-error'],[class*='-error ']",
    "help": ".run-help,.set-help,.sec-note", "icon-bi": "i.bi", "icon-lucide": "svg[x-icon],svg[data-lucide],svg.lucide",
    "rail-btn": ".rail button", "side-search": ".side input", "project-row": ".project-row,.proj-row,.side .row",
    "body-text": "p", "h": "h1,h2,h3,.modal-title,.sec-card-title",
  };
  const vis = (el) => {
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== "hidden" && s.display !== "none";
  };
  const parse = (c) => {
    const m = c.match(/rgba?\(([^)]+)\)/);
    if (!m) return null;
    const p = m[1].split(/[ ,/]+/).filter(Boolean).map(Number);
    return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1];
  };
  const effBg = (el) => {
    const layers = [];
    for (let e = el; e; e = e.parentElement) {
      const c = parse(getComputedStyle(e).backgroundColor);
      if (c && c[3] > 0) { layers.push(c); if (c[3] >= 1) break; }
    }
    let base = [36, 32, 28];
    for (let i = layers.length - 1; i >= 0; i--) {
      const [r, g, b, a] = layers[i];
      base = [r * a + base[0] * (1 - a), g * a + base[1] * (1 - a), b * a + base[2] * (1 - a)];
    }
    return base;
  };
  const lum = ([r, g, b]) => {
    const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  };
  const contrast = (el) => {
    const s = getComputedStyle(el);
    const fg = parse(s.color); if (!fg) return null;
    const bg = effBg(el);
    let op = 1; for (let e = el; e; e = e.parentElement) op *= Number(getComputedStyle(e).opacity);
    const a = fg[3] * op;
    const f = [fg[0] * a + bg[0] * (1 - a), fg[1] * a + bg[1] * (1 - a), fg[2] * a + bg[2] * (1 - a)];
    const L1 = lum(f), L2 = lum(bg);
    return Math.round(((Math.max(L1, L2) + 0.05) / (Math.min(L1, L2) + 0.05)) * 100) / 100;
  };
  const hex = (c) => { const p = parse(c); if (!p) return c; return "#" + p.slice(0, 3).map((v) => Math.round(v).toString(16).padStart(2, "0")).join("") + (p[3] < 1 ? "/" + p[3] : ""); };
  const snap = (el) => {
    const s = getComputedStyle(el); const r = el.getBoundingClientRect();
    return {
      fs: s.fontSize, lh: s.lineHeight, fw: s.fontWeight, ff: s.fontFamily.split(",")[0],
      color: hex(s.color), bg: "#" + effBg(el).map((v) => Math.round(v).toString(16).padStart(2, "0")).join(""),
      border: `${s.borderTopWidth} ${s.borderTopStyle} ${hex(s.borderTopColor)}`, radius: s.borderTopLeftRadius,
      pad: `${s.paddingTop} ${s.paddingRight} ${s.paddingBottom} ${s.paddingLeft}`,
      margin: `${s.marginTop} ${s.marginRight} ${s.marginBottom} ${s.marginLeft}`, gap: s.gap,
      w: Math.round(r.width), h: Math.round(r.height), op: s.opacity, contrast: contrast(el),
      shadow: s.boxShadow === "none" ? "none" : s.boxShadow.slice(0, 60),
    };
  };
  const desc = (el) => el.tagName.toLowerCase() + (el.className && typeof el.className === "string" ? "." + el.className.trim().split(/\s+/).join(".") : "") + (el.getAttribute("title") ? `[${el.getAttribute("title")}]` : "") + " " + (el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 30);
  function all(scope) {
    const roots = scope ? [...document.querySelectorAll(scope)].filter(vis) : [document];
    const res = {};
    for (const [k, sel] of Object.entries(C)) {
      const els = roots.flatMap((r) => [...(r.matches && r.matches(sel) ? [r] : []), ...r.querySelectorAll(sel)]).filter(vis).slice(0, 12);
      if (!els.length) continue;
      const rows = els.map(snap);
      const distinct = {};
      for (const key of Object.keys(rows[0])) {
        const vals = [...new Set(rows.map((x) => String(x[key])))];
        distinct[key] = vals.length === 1 ? vals[0] : vals;
      }
      distinct.n = els.length;
      distinct.minContrast = Math.min(...rows.map((x) => x.contrast ?? 99));
      distinct.sample = desc(els[0]);
      res[k] = distinct;
    }
    return res;
  }
  let rec = [];
  const fsnap = (el) => { const s = getComputedStyle(el); return { o: `${s.outlineStyle} ${s.outlineWidth} ${hex(s.outlineColor)}`, sh: s.boxShadow, bc: s.borderTopColor, bg: s.backgroundColor, c: s.color }; };
  function focusStart() { rec = []; }
  function focusRecord() { const el = document.activeElement; if (el && el !== document.body) rec.push({ el, f: fsnap(el), fv: el.matches(":focus-visible") }); }
  function focusReport() {
    document.activeElement?.blur?.();
    const seen = new Map();
    for (const r of rec) {
      const n = fsnap(r.el);
      const ring = !r.f.o.startsWith("none") && !r.f.o.includes(" 0px");
      const changed = ring || r.f.sh !== n.sh || r.f.bc !== n.bc || r.f.bg !== n.bg || r.f.c !== n.c;
      const key = desc(r.el).split(" ")[0].replace(/\[.*\]/, "");
      const v = seen.get(key) || { n: 0, visible: 0, how: new Set() };
      v.n++; if (changed) v.visible++;
      v.how.add(ring ? "outline " + r.f.o : changed ? "other change" : "NONE");
      seen.set(key, v);
    }
    return [...seen].map(([k, v]) => ({ el: k, n: v.n, visible: v.visible, how: [...v.how] }));
  }
  function targets() {
    const els = [...document.querySelectorAll("button,a[href],input,select,textarea,[role=button],[role=tab],.tab,[tabindex]")].filter(vis);
    const small = {};
    let lt24 = 0, lt44 = 0;
    for (const el of els) {
      const r = el.getBoundingClientRect();
      const m = Math.min(r.width, r.height);
      if (m < 44) lt44++;
      if (m < 24) { lt24++; const k = desc(el).split(" ")[0].replace(/\[.*\]/, ""); small[k] = (small[k] || "") .split(",").filter(Boolean).concat(`${Math.round(r.width)}x${Math.round(r.height)}`).slice(0, 4).join(","); }
    }
    return { total: els.length, lt24, lt44, small };
  }
  function layout() {
    const b = getComputedStyle(document.body); const side = document.querySelector(".side");
    return { cols: b.gridTemplateColumns, sidePos: side && getComputedStyle(side).position, sideVisible: side && vis(side),
      hScroll: document.documentElement.scrollWidth > document.documentElement.clientWidth, bodyFs: b.fontSize };
  }
  function win() {
    const w = document.querySelector(".session-window"); if (!w) return null;
    const parts = {};
    for (const sel of [".session-window", ".session-title", ".session-actions button", ".session-checkout"]) {
      const el = w.matches(sel) ? w : w.querySelector(sel); if (el) parts[sel] = snap(el);
    }
    return parts;
  }
  return { win, all, focusStart, focusRecord, focusReport, targets, layout };
})();
