// The note CARD: a note's view on the consoles stage (ADR-0064 §§2, 7–14).
//
// The split this file rests on: `wb-console.js` owns the desk — the `notes`
// records, the flush, the fold against other pages — and this file owns the
// card: its DOM, its gestures' bindings, its editor, its autosave. Nothing here
// touches `/api/desk`; every placement change goes through
// `WBConsole.saveNotes`, and every byte through `note.read`/`note.write`.
//
// The editor is Milkdown Crepe, vendored lean (ADR-0064 §6) and reached through
// the ONE surface `vendor-build/crepe/entry.js` exports — `window.CrepeLean`.
// Nothing below knows what a ProseMirror plugin is, which is what makes the
// engine replaceable.
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
  // What a new card measures — a post-it, not a document window, but wide
  // enough for the editor's slash menu, which mounts INSIDE the editor and is
  // clipped by a smaller card (measured; see vendor-build/crepe/entry.js).
  const NOTE_DEFAULT = { width: 320, height: 260 };
  // The tones the file's front matter may name (ADR-0064 §3). The stylesheet
  // owns the actual colours; this list is the closed set, mirrored from
  // `note::Color` in the daemon.
  const TONES = ["ochre", "sage", "rose", "slate", "plum", "sand"];
  const DEFAULT_TONE = "sand";
  // The default landing directory (ADR-0064 §4), mirrored from `note::DIR`.
  const DEFAULT_DIR = ".ralphy/notes";
  // Autosave: quiet for this long and the note is written (ADR-0064 §7). Short
  // enough that a closed tab loses a sentence at most, long enough that typing
  // a paragraph is one write and not forty.
  const SAVE_AFTER_MS = 800;

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
    return String(title || "")
      .toLowerCase()
      .normalize("NFKD")
      .replace(/[̀-ͯ]/g, "")
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 48)
      .replace(/-+$/g, "");
  }

  // `note-<UTC yyyymmdd-hhmmss>` — the name a note with no heading yet takes.
  function stampName(now) {
    const d = now instanceof Date ? now : new Date(now ?? Date.now());
    const p = (n) => String(n).padStart(2, "0");
    return (
      "note-" +
      d.getUTCFullYear() +
      p(d.getUTCMonth() + 1) +
      p(d.getUTCDate()) +
      "-" +
      p(d.getUTCHours()) +
      p(d.getUTCMinutes()) +
      p(d.getUTCSeconds())
    );
  }

  // The tone a record/document names, defaulted. Never throws on a tone from a
  // hand-edited file: an unknown name is sand.
  function toneOf(name) {
    return TONES.includes(name) ? name : DEFAULT_TONE;
  }

  // ---- front matter -----------------------------------------------------------
  // Byte-identical to `note::with_color`/`color_of`/`body_of` in the daemon:
  // the two sides write the same header for the same note, so an autosave after
  // a read never rewrites the file for no reason. One field, a closed set — a
  // YAML parser here would be a dependency for `color: sage`.

  function splitFrontMatter(markdown) {
    const text = String(markdown || "");
    const open = text.startsWith("---\n") ? 4 : text.startsWith("---\r\n") ? 5 : 0;
    if (!open) return null;
    const rest = text.slice(open);
    let offset = 0;
    for (const line of rest.split("\n")) {
      if (line.replace(/\r$/, "") === "---") {
        return { inner: rest.slice(0, offset), body: rest.slice(offset + line.length + 1) };
      }
      offset += line.length + 1;
    }
    return null;
  }

  // The document without its front matter — what the editor shows.
  function bodyOf(markdown) {
    const split = splitFrontMatter(markdown);
    return split ? split.body : String(markdown || "");
  }

  // The colour the document names, or `null`.
  function colorOf(markdown) {
    const split = splitFrontMatter(markdown);
    if (!split) return null;
    for (const line of split.inner.split("\n")) {
      const m = /^\s*color:\s*(\S+)\s*$/.exec(line.replace(/\r$/, ""));
      if (m) return TONES.includes(m[1]) ? m[1] : null;
    }
    return null;
  }

  // `markdown` with a front-matter block naming `tone`, replacing one already
  // there. ONE shape, so an autosave never rewrites the header for no reason.
  function withColor(markdown, tone) {
    return `---\ncolor: ${toneOf(tone)}\n---\n${bodyOf(markdown)}`;
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

  // Sleep or wake, as a fold (ADR-0064 §14). A card off-screen for long enough
  // gives its editor back — a Crepe instance is a ProseMirror view, and thirty
  // of them idle on a plane nobody is looking at is the memory the consoles'
  // own dormancy exists to save. The two refusals are the point: a card with
  // unsaved text or a write in flight NEVER sleeps, because tearing the editor
  // down is what would lose it.
  function noteDormancyDecision({ visible, dirty, inFlight, asleep, elapsed, after }) {
    if (visible) return asleep ? "wake" : "stay";
    if (dirty || inFlight) return asleep ? "wake" : "stay";
    if (asleep) return "stay";
    return elapsed >= after ? "sleep" : "stay";
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

  // Paint the lock: the card refuses drag and resize, and the editor goes
  // read-only. The JS guards in `makeDraggable`/`startResize` are the truth for
  // the gestures; this is the glyph and the editor's half.
  function applyLock(el, locked) {
    el._noteLocked = !!locked;
    el.classList.toggle("locked", !!locked);
    const btn = el.querySelector(".note-lock");
    if (btn) {
      btn.textContent = locked ? "🔒" : "🔓";
      btn.title = locked ? "unlock this note" : "lock this note in place";
    }
    try {
      el._noteEditor?.setReadonly(!!locked);
    } catch {}
  }

  // The card's chrome for one record. Mirrors `buildFence`'s construction
  // order, including its hit-test rule.
  function buildCard(record) {
    const el = document.createElement("div");
    el.className = "note-card";
    el.dataset.noteId = record.id;
    el.dataset.tone = DEFAULT_TONE;
    // Everything the card knows that is not in the desk record: the text, what
    // has been written, and what is in flight. Deliberately NOT desk state —
    // the file is the note (ADR-0064 §2).
    el._noteMarkdown = "";
    el._noteTone = DEFAULT_TONE;
    el._noteDirty = false;
    el._noteInFlight = false;
    el._noteTimer = null;

    const head = document.createElement("div");
    head.className = "note-head";
    const grab = document.createElement("span");
    grab.className = "note-grab";
    grab.title = "move this note";
    grab.textContent = "⠿";
    const title = document.createElement("span");
    title.className = "note-title";
    head.append(grab, title);

    const tools = document.createElement("div");
    tools.className = "note-tools";
    // Colour is the FILE's (front matter), so a close and a reopen keep it —
    // which is why this writes the document and not the record.
    const tone = document.createElement("button");
    tone.className = "note-tone";
    tone.type = "button";
    tone.title = "change this note's colour";
    tone.textContent = "◑";
    tone.addEventListener("click", () => cycleTone(el));
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
    tools.append(tone, lock, close);

    const body = document.createElement("div");
    body.className = "note-body";

    const foot = document.createElement("div");
    foot.className = "note-foot";
    const path = document.createElement("span");
    path.className = "note-path";
    // Where an unsaved note will land (ADR-0064 §9: creation has no dialog, the
    // directory is a field in the footer). Gone once the file exists — moving a
    // note is a rename, not a re-aim.
    const dir = document.createElement("input");
    dir.className = "note-dir";
    dir.setAttribute("aria-label", "directory for this note");
    dir.value = DEFAULT_DIR;
    dir.title = "where this note will be saved";
    // The plane's accelerators must not fire on a directory being typed.
    dir.addEventListener("keydown", (e) => e.stopPropagation());
    const state = document.createElement("span");
    state.className = "note-state";
    foot.append(path, dir, state);

    // Every edge and corner resizes; only the SE grip is visible, as a fence's.
    const handles = DIRS.map((d) => {
      const h = document.createElement("div");
      h.className = d === "se" ? "note-edge note-grip" : "note-edge";
      h.dataset.dir = d;
      h.addEventListener(
        "pointerdown",
        window.WBConsole.startResize(el, d, {
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
    installKeys(el);
    installLinks(el);
    // Leaving the card writes what was typed: `focusout` fires before whatever
    // the operator clicked next does anything (ADR-0064 §7).
    el.addEventListener("focusout", () => {
      if (el._noteDirty) flush(el);
    });
    stage()?.append(el);
    return el;
  }

  // ---- the editor --------------------------------------------------------------

  // Mount Crepe over the card's body with `markdown` (front matter stripped —
  // the colour is chrome, not text the operator edits).
  function mountEditor(el, markdown) {
    const body = el.querySelector(".note-body");
    if (!body || !window.CrepeLean) return Promise.resolve(null);
    el._noteMarkdown = markdown;
    el._noteTone = toneOf(colorOf(markdown) || el._noteTone);
    el.dataset.tone = el._noteTone;
    body.textContent = "";
    return window.CrepeLean.create({
      root: body,
      value: bodyOf(markdown),
      readonly: !!el._noteLocked,
      placeholder: "Write a note…",
      onChange: (next) => {
        // Milkdown reports its own value back on mount too; a change that is
        // not a change must not mark the card dirty, or every card would
        // autosave itself once per reload.
        if (next === bodyOf(el._noteMarkdown)) return;
        el._noteMarkdown = withColor(next, el._noteTone);
        markDirty(el);
        paintTitle(el);
      },
    })
      .then((editor) => {
        el._noteEditor = editor;
        paintTitle(el);
        return editor;
      })
      .catch((err) => {
        paintState(el, "editor failed: " + String(err?.message || err));
        return null;
      });
  }

  // Read the note's file into the card. A missing file is a STATE, not a
  // disappearance: the card stays, says so, and offers nothing that would
  // recreate the file behind the operator's back (ADR-0064 §11).
  function loadInto(el, record) {
    if (!record.path) return mountEditor(el, withColor("", el._noteTone));
    return window.WBDaemon.observe(
      "note.read",
      window.WBDaemon.withCheckout({ repo: record.repo, path: record.path }, record.checkout),
    )
      .then((reply) => {
        if (window.WBFail.isError(reply)) {
          paintMissing(el, window.WBFail.message(reply, "could not read this note"));
          return null;
        }
        el.classList.remove("missing");
        return mountEditor(el, reply.markdown || "");
      })
      .catch((err) => {
        paintMissing(el, String(err?.message || err));
        return null;
      });
  }

  function paintMissing(el, reason) {
    el.classList.add("missing");
    const body = el.querySelector(".note-body");
    if (body) {
      body.textContent = "";
      const p = document.createElement("p");
      p.className = "note-gone";
      p.textContent = reason;
      body.append(p);
    }
    paintState(el, reason);
  }

  function paintTitle(el) {
    const title = el.querySelector(".note-title");
    if (title) title.textContent = titleOf(el._noteMarkdown, "Untitled note");
  }

  function paintState(el, text) {
    const state = el.querySelector(".note-state");
    if (state) state.textContent = text || "";
  }

  function markDirty(el) {
    el._noteDirty = true;
    paintState(el, "…");
    clearTimeout(el._noteTimer);
    el._noteTimer = setTimeout(() => {
      el._noteTimer = null;
      flush(el);
    }, SAVE_AFTER_MS);
  }

  // Write now, whatever the timer was going to do. Last-writer-wins by design
  // (ADR-0064 §7): there is one operator and no live sync, so a second page
  // editing the same note resolves by overwriting — and the card that lost
  // re-reads the file on its next wake.
  function flush(el) {
    clearTimeout(el._noteTimer);
    el._noteTimer = null;
    if (!el._noteDirty || el.classList.contains("missing")) return Promise.resolve();
    const record = recordOf(el.dataset.noteId);
    if (!record) return Promise.resolve();
    const markdown = el._noteMarkdown;
    return namePath(el, record, markdown).then((path) => {
      if (!path) return;
      el._noteInFlight = true;
      return window.WBDaemon.write(
        "note.write",
        window.WBDaemon.withCheckout({ repo: record.repo, path, markdown }, record.checkout),
      )
        .then((reply) => {
          el._noteInFlight = false;
          if (window.WBFail.isError(reply)) {
            // The text stays in the editor and the card stays dirty: the next
            // keystroke schedules another attempt, and nothing was lost.
            el.classList.add("danger");
            paintState(el, window.WBFail.message(reply, "not saved"));
            return;
          }
          el.classList.remove("danger");
          // Only for the bytes that landed: a keystroke during the write leaves
          // the card dirty, and the next debounce carries it.
          if (el._noteMarkdown === markdown) el._noteDirty = false;
          paintState(el, "saved");
          setTimeout(() => {
            if (!el._noteDirty) paintState(el, "");
          }, 1200);
        })
        .catch((err) => {
          el._noteInFlight = false;
          el.classList.add("danger");
          paintState(el, String(err?.message || err));
        });
    });
  }

  // The path a save writes to. An already-named note keeps its name for good
  // (ADR-0064 §4): retitling never renames, because a silent rename breaks a
  // link, a backup and a desk record at once. An unnamed one is named HERE,
  // from the title at this moment, stepping past a name already taken.
  function namePath(el, record, markdown) {
    if (record.path) return Promise.resolve(record.path);
    const dirField = el.querySelector(".note-dir");
    const dir = String(dirField?.value || DEFAULT_DIR)
      .trim()
      .replace(/^\/+|\/+$/g, "");
    const base = noteSlug(titleOf(markdown, "")) || stampName();
    return firstFreeName(record, dir, base).then((path) => {
      if (!path) {
        paintState(el, "could not name this note");
        return null;
      }
      patch(record.id, { path });
      paintPath(el, path);
      return path;
    });
  }

  // `<dir>/<base>.note`, or `-2`, `-3`… past one that exists. The probe is a
  // `note.read`: a write would overwrite, and overwriting a note nobody asked
  // to touch is the one thing this must not do.
  const NAME_TRIES = 20;
  function firstFreeName(record, dir, base, n = 1) {
    if (n > NAME_TRIES) return Promise.resolve(null);
    const path = `${dir ? dir + "/" : ""}${base}${n === 1 ? "" : "-" + n}.note`;
    return window.WBDaemon.observe(
      "note.read",
      window.WBDaemon.withCheckout({ repo: record.repo, path }, record.checkout),
    ).then((reply) => {
      const reason = window.WBFail.isError(reply) ? window.WBFail.message(reply, "") : null;
      // Only "not found" means free: "not a note" is a file that exists and is
      // something else, and writing over it would destroy it.
      if (reason === "not found") return path;
      return firstFreeName(record, dir, base, n + 1);
    });
  }

  function paintPath(el, path) {
    const field = el.querySelector(".note-path");
    if (field) {
      field.textContent = path || "";
      field.title = path || "this note has not been saved yet";
    }
    const dir = el.querySelector(".note-dir");
    if (dir) dir.hidden = !!path;
  }

  // The tone cycles through the closed set and is written into the FILE, so a
  // close and a reopen keep it (ADR-0064 §3).
  function cycleTone(el) {
    const next = TONES[(TONES.indexOf(el._noteTone) + 1) % TONES.length];
    el._noteTone = next;
    el.dataset.tone = next;
    el._noteMarkdown = withColor(el._noteMarkdown, next);
    markDirty(el);
  }

  // ---- links, keys, dormancy ----------------------------------------------------

  // A link inside a note obeys the viewer's rule (ADR-0064 §12): an external
  // one opens in a new tab, a repo-relative one is an open REQUEST to the
  // shell. A raw `href` would navigate the whole workbench away.
  function installLinks(el) {
    el.addEventListener("click", (ev) => {
      const a = ev.target?.closest?.("a[href]");
      if (!a || !el.contains(a)) return;
      const record = recordOf(el.dataset.noteId);
      // Resolved against the CHECKOUT ROOT, not the note's own directory: a
      // note lands in `.ralphy/notes/` by default, and a path written there is
      // the operator's path into their repo, not into the run-state directory.
      const target = window.WBViewer?.linkTarget?.("", a.getAttribute("href"));
      if (!target || target.kind === "fragment") {
        ev.preventDefault();
        return;
      }
      if (target.kind === "external") {
        a.target = "_blank";
        a.rel = "noopener";
        return;
      }
      ev.preventDefault();
      document.dispatchEvent(
        new CustomEvent("workbench:open-request", {
          detail: {
            project: record?.repo || null,
            path: target.path,
            fragment: target.fragment,
            checkout: record?.checkout ?? null,
          },
        }),
      );
    });
  }

  // The card's own keys (ADR-0064 §13). `consoleShortcutsBlocked()` already
  // stands down inside a `contentEditable`, so the plane's accelerators do not
  // reach a note being typed into and nothing here has to fight them.
  function installKeys(el) {
    el.addEventListener("keydown", (ev) => {
      if ((ev.ctrlKey || ev.metaKey) && (ev.key === "s" || ev.key === "S")) {
        // The browser's Save-page dialog, over a note the daemon already has,
        // is the wrong answer to this key.
        ev.preventDefault();
        ev.stopPropagation();
        flush(el);
        return;
      }
      if (ev.key !== "Escape") return;
      // Two stages: out of the editor, then off the card. Held at the card so
      // an Escape meant for this edit never also reaches the plane.
      ev.stopPropagation();
      const inEditor = el.querySelector(".note-body")?.contains(document.activeElement);
      document.activeElement?.blur?.();
      if (!inEditor) el.classList.remove("focused");
    });
  }

  // Dormancy (ADR-0064 §14). Its OWN observer, not the consoles': a console
  // sleeps by dropping a socket, a card by destroying an editor, and the two
  // have different refusals — a card with unsaved text never sleeps.
  let observer = null;
  let sweeper = null;
  const watched = new Map();
  const SWEEP_MS = 5000;
  function trackDormancy(el) {
    if (typeof window.IntersectionObserver !== "function") return;
    const root = document.getElementById("workspace");
    if (!root) return;
    if (!observer) {
      observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            const seen = watched.get(entry.target);
            if (!seen) continue;
            seen.visible = entry.isIntersecting;
            seen.since = Date.now();
            if (entry.isIntersecting) wakeCard(entry.target);
          }
        },
        { root, rootMargin: `${window.WBConsole?.DORMANT_MARGIN_PX ?? 300}px` },
      );
    }
    if (sweeper == null) sweeper = setInterval(sweepDormancy, SWEEP_MS);
    watched.set(el, { visible: true, since: Date.now() });
    observer.observe(el);
  }
  function untrackDormancy(el) {
    watched.delete(el);
    observer?.unobserve(el);
  }
  function sweepDormancy() {
    const after = window.WBConsole?.DORMANT_AFTER_MS ?? 15000;
    for (const [el, seen] of [...watched]) {
      if (!el.isConnected) {
        untrackDormancy(el);
        continue;
      }
      const verdict = noteDormancyDecision({
        visible: seen.visible,
        dirty: !!el._noteDirty,
        inFlight: !!el._noteInFlight,
        asleep: !!el._noteAsleep,
        elapsed: Date.now() - seen.since,
        after,
      });
      if (verdict === "sleep") sleepCard(el);
    }
  }
  function sleepCard(el) {
    if (el._noteAsleep || el._noteDirty || el._noteInFlight) return;
    el._noteAsleep = true;
    const editor = el._noteEditor;
    el._noteEditor = null;
    try {
      editor?.destroy();
    } catch {}
    const body = el.querySelector(".note-body");
    if (body) {
      body.textContent = "";
      const p = document.createElement("p");
      p.className = "note-asleep";
      p.textContent = titleOf(el._noteMarkdown, "Untitled note");
      body.append(p);
    }
  }
  function wakeCard(el) {
    if (!el._noteAsleep) return;
    el._noteAsleep = false;
    const record = recordOf(el.dataset.noteId);
    // Re-READ rather than remount the text held here: another page may have
    // written the file while this card slept, and the file is the note.
    if (record) loadInto(el, record);
  }

  // ---- creating, closing, rendering ---------------------------------------------

  // A new note (ADR-0064 §9): no dialog. The card lands in the middle of the
  // view with an empty editor and a default directory in its footer, and NO
  // file — the first autosave names it, so a note nobody typed into is never
  // written.
  function create({ repo, checkout, viewport, offset } = {}) {
    if (!repo) return null;
    const records = window.WBConsole?.notes?.() || [];
    const record = {
      id: `note-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
      repo,
      checkout: checkout ?? null,
      path: "",
      rect: spawnRect(
        viewport || { width: 0, height: 0 },
        offset || { left: 0, top: 0 },
        records.length,
      ),
      locked: false,
      ts: Date.now(),
    };
    window.WBConsole.saveNotes(records.concat([record]));
    render();
    const el = cardEl(record.id);
    if (el) window.WBConsole.focusWin(el);
    return record.id;
  }

  // Close the card, keep the file (ADR-0064 §11). The undo is what makes this
  // safe to do with one click on a document nobody asked to delete.
  function closeCard(id) {
    const record = recordOf(id);
    if (!record) return;
    const el = cardEl(id);
    // Whatever was typed lands before the card goes: a close is not a discard.
    const written = el && el._noteDirty ? flush(el) : Promise.resolve();
    written.then(() => {
      window.WBConsole.saveNotes((window.WBConsole.notes() || []).filter((n) => n.id !== id));
      render();
      window.WBConsole.toast({
        text: record.path ? `note closed · ${record.path} kept` : "note closed",
        action: "Undo",
        onAction: () => {
          window.WBConsole.saveNotes(
            (window.WBConsole.notes() || []).concat([
              // The PATH as it is now, not as it was when the card was built:
              // the close may have been the note's first save.
              { ...(recordOf(id) || record), ...record, ts: Date.now() },
            ]),
          );
          render();
        },
      });
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
      let el = nodes.get(record.id);
      if (!el) {
        el = buildCard(record);
        loadInto(el, record);
        trackDormancy(el);
      }
      paint(el, record, fences);
    }
    for (const [id, el] of nodes) {
      if (seen.has(id)) continue;
      // A card leaving the stage takes its editor with it; the record it was
      // built from has already gone (closed) or moved (detached).
      untrackDormancy(el);
      try {
        el._noteEditor?.destroy();
      } catch {}
      el.remove();
    }
  }

  // One record onto one card: rect, title, path, lock.
  function paint(el, record, fences) {
    const r = record.rect || {};
    el.style.left = (r.left || 0) + "px";
    el.style.top = (r.top || 0) + "px";
    el.style.width = (r.width || NOTE_DEFAULT.width) + "px";
    el.style.height = (r.height || NOTE_DEFAULT.height) + "px";
    paintTitle(el);
    paintPath(el, record.path);
    applyLock(el, !!lockedBy(record, fences));
  }

  // The popup's side of a detached fence: the same card, from the snapshot the
  // opener handed over.
  function mountDetached(record) {
    const el = buildCard(record);
    loadInto(el, record);
    paint(el, record, []);
    return el;
  }

  // Every card writes what it holds before the page goes. A best effort by
  // definition — the socket may not finish — but an 800 ms debounce loses a
  // sentence without it, and this is the same bargain `wb-console.js` takes
  // for the desk on `pagehide`.
  function flushAll() {
    const st = stage();
    if (!st) return;
    for (const el of st.querySelectorAll(".note-card")) {
      if (el._noteDirty) flush(el);
    }
  }

  return {
    // folds
    titleOf,
    anchorsOf,
    noteSlug,
    stampName,
    toneOf,
    bodyOf,
    colorOf,
    withColor,
    lockedBy,
    spawnRect,
    noteDormancyDecision,
    TONES,
    DEFAULT_TONE,
    DEFAULT_DIR,
    NOTE_MIN,
    NOTE_DEFAULT,
    SAVE_AFTER_MS,
    // the card
    render,
    create,
    buildCard,
    mountDetached,
    applyLock,
    persistCards,
    closeCard,
    isAway,
    cardEl,
    flushAll,
  };
})();
