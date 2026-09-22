// The note CARD: a note's view on the consoles stage (ADR-0064 §§2, 8).
//
// The split this file rests on: `wb-console.js` owns the desk — the `notes`
// records, the flush, the fold against other pages — and this file owns the
// card: its DOM, its gestures' bindings, its lock, its close. Nothing here
// touches `/api/desk`; everything goes through `WBConsole.saveNotes`, which is
// the one write path. The editor, autosave and the anchors land in the next
// slices; this one places the card and keeps it placed.
//
// Loaded in the shell AND in the detached-fence popup, so: no module-scope DOM
// read, no `fetch`, no timer at load (the ui-tests evaluate this file under a
// stub document), and no browser store of its own — `wb-view.js` holds the one
// there is (#339).
window.WBNotes = (function () {
  "use strict";

  // The card's floor. Below a console's minimum on purpose: a note is often a
  // three-line reminder, and forcing it to a console's footprint would make
  // the stage unreadable.
  const NOTE_MIN = { width: 160, height: 120 };
  // What a new card measures — a post-it, not a document window.
  const NOTE_DEFAULT = { width: 260, height: 200 };
  // The tones the file's front matter may name (ADR-0064 §3). The stylesheet
  // owns the actual colours; this list is the closed set, mirrored from
  // `note::Color` in the daemon.
  const TONES = ["ochre", "sage", "rose", "slate", "plum", "sand"];
  const DEFAULT_TONE = "sand";

  // ---- pure folds (tested without a document) ---------------------------------

  // A note's title is its first `#` heading, and nothing else is a title
  // (ADR-0064 §3: one source of truth for the name). An untitled note is
  // called what the card calls it.
  function titleOf(markdown, fallback) {
    const lines = String(markdown || "").split("\n");
    for (const line of lines) {
      const m = /^#\s+(.+?)\s*$/.exec(line);
      if (m) return m[1];
    }
    return fallback === undefined ? "Untitled note" : fallback;
  }

  // The `##` headings, in document order — the jump anchors (ADR-0064 §10).
  // `#` is the title and `###` and below are structure inside a section; only
  // the second level is an anchor.
  function anchorsOf(markdown) {
    const out = [];
    const lines = String(markdown || "").split("\n");
    let fenced = false;
    for (const line of lines) {
      // A `##` inside a code fence is source, not a heading.
      if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
      if (fenced) continue;
      const m = /^##\s+(.+?)\s*$/.exec(line);
      if (m) out.push({ text: m[1], index: out.length });
    }
    return out;
  }

  // The filename a first save derives from the title (ADR-0064 §4). Lowercase,
  // ASCII-ish, hyphen-joined, capped — and `""` when the title yields nothing,
  // which is the caller's signal to fall back to a stamp.
  function noteSlug(title) {
    const slug = String(title || "")
      .toLowerCase()
      .normalize("NFKD")
      .replace(/[̀-ͯ]/g, "")
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 48)
      .replace(/-+$/g, "");
    return slug;
  }

  // The tone a record/document names, defaulted. Never throws on a tone from a
  // hand-edited file: an unknown name is sand.
  function toneOf(name) {
    return TONES.includes(name) ? name : DEFAULT_TONE;
  }

  // Is this card read-only? Its own `locked`, or the lock of the fence that
  // holds it — the same derivation a console's lock uses (ADR-0051 §6), which
  // is why the fence's lock is NOT copied onto the record.
  function lockedBy(record, fences) {
    if (record?.locked) return "self";
    const held = window.WBGeometry?.fenceOf?.(fences || [], record?.rect || {});
    return held?.locked ? "fence" : null;
  }

  // Where a new card lands: the centre of what the operator is looking at,
  // nudged by how many cards already sit there so two `New note`s in a row do
  // not stack perfectly.
  function spawnRect(viewport, offset, taken) {
    const step = 24 * ((taken || 0) % 6);
    const left = Math.max(
      0,
      Math.round((offset?.left || 0) + (viewport?.width || 0) / 2 - NOTE_DEFAULT.width / 2) + step,
    );
    const top = Math.max(
      0,
      Math.round((offset?.top || 0) + (viewport?.height || 0) / 2 - NOTE_DEFAULT.height / 2) + step,
    );
    return { left, top, width: NOTE_DEFAULT.width, height: NOTE_DEFAULT.height };
  }

  // ---- the card ----------------------------------------------------------------

  const DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

  function stage() {
    return document.getElementById("stage");
  }

  function cardEl(id) {
    const st = stage();
    if (!st) return null;
    // Indexed, not selected: an id is daemon data and one quote in it would
    // throw a SyntaxError out of the whole render (`renderFences`' lesson).
    for (const el of st.querySelectorAll(".note-card")) {
      if (el.dataset.noteId === id) return el;
    }
    return null;
  }

  function recordOf(id) {
    return (window.WBConsole?.notes?.() || []).find((n) => n.id === id) || null;
  }

  // Write one card's record back, keeping the rest of the collection. Every
  // mutation of a card goes through here, so `ts` is stamped in exactly one
  // place and the daemon's fold always has a newer record to prefer.
  function patch(id, fields) {
    const next = (window.WBConsole?.notes?.() || []).map((n) =>
      n.id === id ? { ...n, ...fields, ts: Date.now() } : n,
    );
    window.WBConsole?.saveNotes(next);
  }

  // The rect as the DOM holds it — the shape the desk record wants.
  function rectOf(el) {
    return {
      left: el.offsetLeft,
      top: el.offsetTop,
      width: el.offsetWidth,
      height: el.offsetHeight,
    };
  }

  // Persist the placement of one or more cards after a gesture: ONE write for
  // the whole set, so a fence carrying six cards uploads once.
  function persistCards(els) {
    const list = Array.isArray(els) ? els : [els];
    const moved = new Map(list.filter(Boolean).map((el) => [el.dataset.noteId, rectOf(el)]));
    if (!moved.size) return;
    const now = Date.now();
    const next = (window.WBConsole?.notes?.() || []).map((n) =>
      moved.has(n.id) ? { ...n, rect: moved.get(n.id), ts: now } : n,
    );
    window.WBConsole?.saveNotes(next);
  }

  // Paint the lock: the card refuses drag and resize, and says so. The JS
  // guards in `makeDraggable`/`startResize` are the truth; this is the glyph.
  function applyLock(el, locked) {
    el._noteLocked = !!locked;
    el.classList.toggle("locked", !!locked);
    const btn = el.querySelector(".note-lock");
    if (btn) {
      btn.textContent = locked ? "🔒" : "🔓";
      btn.title = locked ? "unlock this note" : "lock this note in place";
    }
  }

  // The card's chrome for one record. Mirrors `buildFence`'s construction
  // order, including its hit-test rule.
  function buildCard(record) {
    const el = document.createElement("div");
    el.className = "note-card";
    el.dataset.noteId = record.id;
    el.dataset.tone = toneOf(record.tone);

    const head = document.createElement("div");
    head.className = "note-head";
    const grab = document.createElement("span");
    grab.className = "note-grab";
    grab.title = "move this note";
    grab.textContent = "⠿";
    const title = document.createElement("span");
    title.className = "note-title";
    const tools = document.createElement("div");
    tools.className = "note-tools";
    const lock = document.createElement("button");
    lock.className = "note-lock";
    lock.type = "button";
    lock.addEventListener("click", () => {
      const r = recordOf(el.dataset.noteId);
      patch(el.dataset.noteId, { locked: !r?.locked });
      render();
    });
    const close = document.createElement("button");
    close.className = "note-close";
    close.type = "button";
    close.title = "close this note (the file is kept)";
    close.textContent = "×";
    close.addEventListener("click", () => closeCard(el.dataset.noteId));
    tools.append(lock, close);
    head.append(grab, title);

    const body = document.createElement("div");
    body.className = "note-body";

    const foot = document.createElement("div");
    foot.className = "note-foot";
    const path = document.createElement("span");
    path.className = "note-path";
    foot.append(path);

    // Every edge and corner resizes; only the SE grip is visible, as a fence's.
    const handles = DIRS.map((dir) => {
      const h = document.createElement("div");
      h.className = dir === "se" ? "note-edge note-grip" : "note-edge";
      h.dataset.dir = dir;
      h.addEventListener(
        "pointerdown",
        window.WBConsole.startResize(el, dir, {
          locked: () => !!el._noteLocked,
          onDrop: () => persistCards(el),
          min: NOTE_MIN,
        }),
      );
      return h;
    });

    // ORDER IS THE HIT TEST (see `buildFence`): the bands overlap the head and
    // the tools, later siblings win, so the interactive clusters go last.
    el.append(...handles, head, body, foot, tools);
    el.addEventListener("pointerdown", () => window.WBConsole.focusWin(el), true);
    window.WBConsole.makeDraggable(el, head, {
      locked: () => !!el._noteLocked,
      onDrop: () => persistCards(el),
    });
    stage()?.append(el);
    return el;
  }

  // Close the card, keep the file (ADR-0064 §11). The undo is what makes this
  // safe to do with one click on a document nobody asked to delete.
  function closeCard(id) {
    const record = recordOf(id);
    if (!record) return;
    window.WBConsole.saveNotes((window.WBConsole.notes() || []).filter((n) => n.id !== id));
    render();
    window.WBConsole.toast({
      text: record.path ? `note closed · ${record.path} kept` : "note closed",
      action: "Undo",
      onAction: () => {
        window.WBConsole.saveNotes(
          (window.WBConsole.notes() || []).concat([{ ...record, ts: Date.now() }]),
        );
        render();
      },
    });
  }

  // A card whose fence is detached is not on THIS stage — it is in the popup.
  // Derived, never stored: the record stays in the desk (the popup is a view,
  // not an owner), so a reload works this out again from the fence registry
  // instead of restoring a set that died with the document.
  function isAway(record, fences) {
    const held = window.WBGeometry?.fenceOf?.(fences || [], record?.rect || {});
    return !!held && !!window.WBConsole?.isDetached?.(held.id);
  }

  // Put the desk's cards on the stage: build what is missing, move what moved,
  // drop what is gone. Idempotent — the same shape as `renderFences`.
  function render() {
    const st = stage();
    if (!st) return;
    const records = window.WBConsole?.notes?.() || [];
    const fences = window.WBConsole?.fenceRecords?.() || [];
    const nodes = new Map();
    for (const el of st.querySelectorAll(".note-card")) nodes.set(el.dataset.noteId, el);
    const seen = new Set();
    for (const record of records) {
      if (isAway(record, fences)) continue;
      seen.add(record.id);
      const el = nodes.get(record.id) || buildCard(record);
      paint(el, record, fences);
    }
    for (const [id, el] of nodes) {
      if (!seen.has(id)) el.remove();
    }
  }

  // One record onto one card: rect, tone, title, path, lock.
  function paint(el, record, fences) {
    const r = record.rect || {};
    el.style.left = (r.left || 0) + "px";
    el.style.top = (r.top || 0) + "px";
    el.style.width = (r.width || NOTE_DEFAULT.width) + "px";
    el.style.height = (r.height || NOTE_DEFAULT.height) + "px";
    el.dataset.tone = toneOf(record.tone || el._noteTone);
    const title = el.querySelector(".note-title");
    if (title) title.textContent = el._noteTitle || titleOf(el._noteMarkdown, "Untitled note");
    const path = el.querySelector(".note-path");
    if (path) {
      path.textContent = record.path || "unsaved";
      path.title = record.path || "this note has not been saved yet";
    }
    applyLock(el, !!lockedBy(record, fences));
  }

  // The popup's side of a detached fence: the same card, built from the
  // snapshot the opener handed over and translated into this window.
  function mountDetached(record) {
    const el = buildCard(record);
    paint(el, record, []);
    return el;
  }

  return {
    // folds
    titleOf,
    anchorsOf,
    noteSlug,
    toneOf,
    lockedBy,
    spawnRect,
    TONES,
    DEFAULT_TONE,
    NOTE_MIN,
    NOTE_DEFAULT,
    // the card
    render,
    buildCard,
    mountDetached,
    applyLock,
    persistCards,
    closeCard,
    isAway,
    cardEl,
  };
})();
