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
  const NOTE_MIN = { width: 160, height: 100 };
  // What a new card measures — a post-it, not a document window, but wide
  // enough for the editor's slash menu, which mounts INSIDE the editor and is
  // clipped by a smaller card (measured; see vendor-build/crepe/entry.js).
  const NOTE_DEFAULT = { width: 320, height: 260 };
  // The tones the file's front matter may name (ADR-0064 §3). The stylesheet
  // owns the actual colours; this list is the closed set. The daemon reads no
  // field of the front matter — its codec carries bytes — so the shell is the
  // one side that has to agree with itself.
  const TONES = ["ochre", "sage", "rose", "slate", "plum", "sand"];
  const DEFAULT_TONE = "sand";
  // How much of the tone the card's GROUND takes. `wash` is ADR-0064 §8's
  // quiet tint; `solid` is the tone itself, which is what makes "a yellow note
  // with white text" possible at all — a 10 % tint over a dark ground is not a
  // colour anyone would call yellow. The stylesheet owns both mixes.
  const FILLS = ["wash", "solid"];
  const DEFAULT_FILL = "wash";
  // The ink, chosen BESIDE the ground so the pair is the operator's: the
  // theme's own text, a near-white and a near-black for the solid fills, and
  // the six tones again for a coloured hand. Unknown names fall back like a
  // tone's.
  const INKS = ["default", "light", "dark"].concat(TONES);
  const DEFAULT_INK = "default";
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
  function fillOf(name) {
    return FILLS.includes(name) ? name : DEFAULT_FILL;
  }
  function inkOf(name) {
    return INKS.includes(name) ? name : DEFAULT_INK;
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

  // One field out of the block, by name, or `null` when it is absent or names
  // something outside its closed set.
  function fieldOf(markdown, key, set) {
    const split = splitFrontMatter(markdown);
    if (!split) return null;
    const re = new RegExp("^\\s*" + key + ":\\s*(\\S+)\\s*$");
    for (const line of split.inner.split("\n")) {
      const m = re.exec(line.replace(/\r$/, ""));
      if (m) return set.includes(m[1]) ? m[1] : null;
    }
    return null;
  }

  // The colour the document names, or `null`.
  function colorOf(markdown) {
    return fieldOf(markdown, "color", TONES);
  }

  // The whole look of the card, defaulted: the ground's tone, how much of it
  // the ground takes, and the ink over it (ADR-0064 §8, amended 2026-09-22).
  function styleOf(markdown) {
    return {
      tone: toneOf(colorOf(markdown) || DEFAULT_TONE),
      fill: fillOf(fieldOf(markdown, "fill", FILLS) || DEFAULT_FILL),
      ink: inkOf(fieldOf(markdown, "ink", INKS) || DEFAULT_INK),
    };
  }

  // `markdown` under a front-matter block carrying `style`, replacing one
  // already there. ONE shape, so an autosave never rewrites the header for no
  // reason — and the two defaults are OMITTED, so a note nobody restyled
  // carries the same single `color:` line it always did.
  function withStyle(markdown, style) {
    const tone = toneOf(style?.tone);
    const fill = fillOf(style?.fill);
    const ink = inkOf(style?.ink);
    const lines = [`color: ${tone}`];
    if (fill !== DEFAULT_FILL) lines.push(`fill: ${fill}`);
    if (ink !== DEFAULT_INK) lines.push(`ink: ${ink}`);
    return `---\n${lines.join("\n")}\n---\n${bodyOf(markdown)}`;
  }

  // The visible name, as a WRITE (ADR-0064 §4: the title is the first `#`
  // heading, so retitling is editing that heading and nothing else — the file
  // keeps its name, which is what `⋯ → Rename file…` is for). No heading yet:
  // one is inserted ahead of the body.
  function withTitle(markdown, title) {
    const name = String(title || "").replace(/[\r\n]+/g, " ").trim();
    const body = bodyOf(markdown);
    // The SAME scan `titleOf` runs, line by line and in its order: whatever it
    // reports as the title is the line this rewrites, or the two would
    // disagree about which heading names the note.
    const lines = body.split("\n");
    const at = lines.findIndex((line) => /^#\s+(.+?)\s*$/.test(line));
    if (at >= 0) {
      // An empty name UNTITLES the note: the heading goes, rather than leaving
      // a bare `#` that renders as an empty h1.
      lines.splice(at, 1, ...(name ? ["# " + name] : []));
      return withStyle(lines.join("\n"), styleOf(markdown));
    }
    if (!name) return withStyle(body, styleOf(markdown));
    return withStyle(`# ${name}\n\n${body}`, styleOf(markdown));
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
  // `fences` is optional: given, the cascade steps PAST a slot whose centre
  // lands inside one, because a note is born outside any fence (ADR-0064 §11)
  // — a card that opened inside a locked fence would be read-only the moment
  // it appeared. A plane where every slot is fenced falls back to the first:
  // somewhere visible beats nowhere.
  const SPAWN_SLOTS = 6;
  function spawnRect(viewport, offset, taken, fences) {
    const at = (n) => {
      const step = 24 * (n % SPAWN_SLOTS);
      return {
        left: Math.max(
          0,
          Math.round((offset?.left || 0) + (viewport?.width || 0) / 2 - NOTE_DEFAULT.width / 2) +
            step,
        ),
        top: Math.max(
          0,
          Math.round((offset?.top || 0) + (viewport?.height || 0) / 2 - NOTE_DEFAULT.height / 2) +
            step,
        ),
        width: NOTE_DEFAULT.width,
        height: NOTE_DEFAULT.height,
      };
    };
    const start = taken || 0;
    const first = at(start);
    if (!fences || !fences.length) return first;
    for (let i = 0; i < SPAWN_SLOTS; i++) {
      const rect = at(start + i);
      if (!window.WBGeometry?.fenceOf?.(fences, rect)) return rect;
    }
    return first;
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
    // A lock is a state, not a moment: an open field or palette over a card
    // that just went read-only would write on its next keystroke.
    if (locked) {
      endTitle(el);
      if (openPalette && el.contains(openPalette)) closePalette();
    }
    const btn = el.querySelector(".note-lock");
    if (btn) {
      // The console's own two glyphs, verbatim (`wb-console.js`'s `applyLock`):
      // one lock on the plane, not one per surface.
      btn.innerHTML = locked ? '<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>';
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
    el.dataset.fill = DEFAULT_FILL;
    el.dataset.ink = DEFAULT_INK;
    // Everything the card knows that is not in the desk record: the text, what
    // has been written, and what is in flight. Deliberately NOT desk state —
    // the file is the note (ADR-0064 §2).
    el._noteMarkdown = "";
    el._noteTone = DEFAULT_TONE;
    el._noteFill = DEFAULT_FILL;
    el._noteInk = DEFAULT_INK;
    el._noteDirty = false;
    el._noteInFlight = false;
    el._noteTimer = null;

    const head = document.createElement("div");
    head.className = "note-head";
    const grab = document.createElement("span");
    grab.className = "note-grab";
    grab.title = "move this note";
    // Bootstrap Icons, like every other control on the plane: the card's
    // chrome was the one surface drawing its controls as text characters, and
    // a braille-dots grip beside a console's `bi-grip-vertical` reads as a
    // different application. The console's titlebar is the reference
    // (`buildChrome`), and a test pins the characters back OUT.
    grab.innerHTML = '<i class="bi bi-grip-vertical"></i>';
    const title = document.createElement("span");
    title.className = "note-title";
    title.title = "click to rename this note";
    // Renaming IS editing the first `#` heading (ADR-0064 §4) — the title has
    // no storage of its own, so this field writes the document. An INPUT and
    // not `contenteditable`: the head is a drag handle, and a caret inside a
    // handle is two gestures on one pixel.
    const titleEdit = document.createElement("input");
    titleEdit.className = "note-title-edit";
    titleEdit.setAttribute("aria-label", "this note's title");
    titleEdit.hidden = true;
    titleEdit.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.key === "Enter") commitTitle(el);
      else if (ev.key === "Escape") endTitle(el);
    });
    titleEdit.addEventListener("blur", () => commitTitle(el));
    // A PRESS THAT DID NOT MOVE opens the field, and that shape is the whole
    // of it. The title is `flex: 1` and therefore most of the head, which is
    // the drag handle: swallowing its `pointerdown` (the obvious way to stop a
    // rename from arming a drag) left the card draggable only by the 10 px of
    // grip beside it — MEASURED, a drag across the title moved nothing. So
    // nothing is stopped on the way down, and the decision is taken on the way
    // up, against the same 3 px the gesture calls a drag.
    //
    // The `mousedown` default IS cancelled, and that is load-bearing for the
    // other half — MEASURED: it moves focus to the nearest focusable ancestor,
    // which blurred the field this press had just opened, and `blur` commits,
    // so the field closed inside the same click and the rename never happened.
    let press = null;
    title.addEventListener("pointerdown", (ev) => {
      press = { x: ev.clientX, y: ev.clientY };
    });
    title.addEventListener("mousedown", (ev) => ev.preventDefault());
    title.addEventListener("pointerup", (ev) => {
      const from = press;
      press = null;
      if (!from) return;
      if (Math.abs(ev.clientX - from.x) > 3 || Math.abs(ev.clientY - from.y) > 3) return;
      beginTitle(el);
    });
    head.append(grab, title, titleEdit);

    const tools = document.createElement("div");
    tools.className = "note-tools";
    // Colour is the FILE's (front matter), so a close and a reopen keep it —
    // which is why this writes the document and not the record.
    const tone = document.createElement("button");
    tone.className = "note-tone";
    tone.type = "button";
    tone.title = "this note's ground and ink";
    tone.innerHTML = '<i class="bi bi-palette"></i>';
    tone.addEventListener("click", (ev) => {
      ev.stopPropagation();
      togglePalette(el);
    });
    const lock = document.createElement("button");
    lock.className = "note-lock";
    lock.type = "button";
    lock.addEventListener("click", () => {
      const r = recordOf(el.dataset.noteId);
      patch(el.dataset.noteId, { locked: !r?.locked });
      render();
    });
    const more = document.createElement("button");
    more.className = "note-more";
    more.type = "button";
    more.title = "rename or delete this note's file";
    more.innerHTML = '<i class="bi bi-three-dots"></i>';
    more.addEventListener("click", (ev) => {
      ev.stopPropagation();
      toggleMenu(el);
    });
    const close = document.createElement("button");
    close.className = "note-close";
    close.type = "button";
    close.title = "close this note (the file is kept)";
    close.innerHTML = '<i class="bi bi-x-lg"></i>';
    close.addEventListener("click", () => closeCard(el.dataset.noteId));
    tools.append(tone, more, lock, close);

    // The file actions (ADR-0064 §11), in a menu rather than in the head: they
    // act on the FILE, not on the card, and two more glyphs beside the close
    // button is where a slip becomes a deletion.
    const menu = document.createElement("div");
    menu.className = "note-menu-card";
    menu.hidden = true;
    const renameItem = document.createElement("button");
    renameItem.className = "note-menu-item";
    renameItem.type = "button";
    renameItem.textContent = "Rename file…";
    renameItem.addEventListener("click", () => {
      menu.hidden = true;
      renameNote(el);
    });
    const drop = document.createElement("button");
    drop.className = "note-menu-item danger";
    drop.type = "button";
    drop.textContent = "Delete file…";
    drop.addEventListener("click", () => {
      menu.hidden = true;
      deleteNote(el);
    });
    const point = document.createElement("button");
    point.className = "note-menu-item";
    point.type = "button";
    point.textContent = "Point elsewhere…";
    point.addEventListener("click", () => {
      menu.hidden = true;
      pointElsewhere(el);
    });
    menu.append(renameItem, point, drop);

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
    // The rename field (ADR-0064 §11), in the footer beside the path it
    // replaces while an edit is open. An INPUT and not `window.prompt`: the
    // native dialog is dismissed by default in an automated browser, which is
    // exactly the objection `wb-console.js` records against `window.confirm`.
    const rename = document.createElement("input");
    rename.className = "note-rename";
    rename.setAttribute("aria-label", "new file name for this note");
    rename.hidden = true;
    rename.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.key === "Enter") commitRename(el);
      else if (ev.key === "Escape") endRename(el);
    });
    rename.addEventListener("blur", () => endRename(el));
    const state = document.createElement("span");
    state.className = "note-state";
    foot.append(path, dir, rename, state);

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
    const palette = buildPalette(el);
    el.append(...handles, head, body, foot, tools, menu, palette);
    el.addEventListener("pointerdown", () => window.WBConsole.focusWin(el), true);
    window.WBConsole.makeDraggable(el, head, {
      locked: () => !!el._noteLocked,
      onDrop: () => persistCards(el),
    });
    // A press on the body that misses the editor still means "type here".
    // MEASURED: the editor fills the body but not its padding, and a short
    // note leaves most of a card below the last line — a click there focused
    // nothing, and the operator's next keystroke went wherever the focus
    // happened to be. The card is a place to write; every pixel of its body
    // says so.
    body.addEventListener("mousedown", (ev) => {
      const dom = el._noteEditor?.dom?.();
      if (!dom || ev.target !== body || el._noteLocked) return;
      ev.preventDefault();
      try {
        dom.focus();
      } catch {}
    });
    installKeys(el);
    installLinks(el);
    // Leaving the card writes what was typed: `focusout` fires before whatever
    // the operator clicked next does anything (ADR-0064 §7).
    el.addEventListener("focusout", () => {
      // Unconditional: `flush` asks the editor what it holds and returns at
      // once when there is nothing to write, so the last character typed
      // before the pointer left the body is carried even though the editor's
      // change listener has not fired yet.
      flush(el);
    });
    stage()?.append(el);
    // In the WINDOW tier from the moment it exists. A card built by a restore
    // is never focused, so without this it kept `z-index: auto` and every
    // console painted over it — see `WBConsole.stackWin`.
    window.WBConsole.stackWin?.(el);
    return el;
  }

  // ---- the editor --------------------------------------------------------------

  // The editor answers the BODY — it never sees the front matter — so every
  // value that comes back out of it is dressed in the card's look before it
  // becomes the document. Reading the look off the card and not off `next` is
  // the point: `styleOf("")` is the default, and dressing an editor's answer
  // with that would silently reset a restyled note on the first keystroke.
  function dress(el, body) {
    return withStyle(body, { tone: el._noteTone, fill: el._noteFill, ink: el._noteInk });
  }

  // Mount Crepe over the card's body with `markdown` (front matter stripped —
  // the look is chrome, not text the operator edits). Re-entrant: a retitle
  // hands the editor a document it did not write, so the live view is given
  // back FIRST — two ProseMirror views over one body is the leak dormancy
  // exists to prevent, arriving by another door.
  function mountEditor(el, markdown) {
    const body = el.querySelector(".note-body");
    if (!body || !window.CrepeLean) return Promise.resolve(null);
    const previous = el._noteEditor;
    el._noteEditor = null;
    try {
      previous?.destroy();
    } catch {}
    el._noteMarkdown = markdown;
    // A card with an editor is AWAKE by definition — `wakeCard` is not the
    // only door any more, a retitle remounts too, and a card left flagged
    // asleep with a live view would never be given back again.
    el._noteAsleep = false;
    const style = styleOf(markdown);
    el._noteTone = style.tone;
    el._noteFill = style.fill;
    el._noteInk = style.ink;
    el.dataset.tone = style.tone;
    el.dataset.fill = style.fill;
    el.dataset.ink = style.ink;
    body.textContent = "";
    // Marked here and cleared on teardown: `create()` resolves a tick or two
    // later, and a card closed, detached or evicted in between would otherwise
    // leave a live ProseMirror view with its listeners on a detached node —
    // the exact leak dormancy exists to prevent.
    el._noteGone = false;
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
        el._noteMarkdown = dress(el, next);
        markDirty(el);
        paintTitle(el);
      },
    })
      .then((editor) => {
        if (el._noteGone) {
          try {
            editor?.destroy();
          } catch {}
          return null;
        }
        el._noteEditor = editor;
        paintTitle(el);
        if (el._noteFocusOnMount) {
          el._noteFocusOnMount = false;
          try {
            editor?.dom()?.focus();
          } catch {}
        }
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
    if (!record.path) return mountEditor(el, dress(el, ""));
    return window.WBDaemon.observe(
      "note.read",
      window.WBDaemon.withCheckout({ repo: record.repo, path: record.path }, record.checkout),
    )
      .then((reply) => {
        if (window.WBFail.isError(reply)) {
          const reason = window.WBFail.message(reply, "could not be read");
          // A file that is not ours is not a missing note — it is someone
          // else's file under our extension (ADR-0064 §11). The card goes and
          // the shell says so; keeping it would put an editor over bytes and
          // the first autosave would overwrite them.
          if (reason === "not a note") {
            window.WBConsole.saveNotes(
              (window.WBConsole.notes() || []).filter((n) => n.id !== record.id),
            );
            render();
            document.dispatchEvent(
              new CustomEvent("workbench:open-request", {
                detail: {
                  project: record.repo,
                  path: record.path,
                  checkout: record.checkout ?? null,
                  as: "bytes",
                },
              }),
            );
            return null;
          }
          // The PATH is half the message: a card says which file it lost, or
          // the operator is left guessing which of six notes this one was.
          paintMissing(el, `${record.path} — ${reason}`);
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

  // Ask the EDITOR what it holds, rather than trusting that its change
  // notification has arrived. MEASURED: Milkdown's `markdownUpdated` lands a
  // tick or more after the keystroke, so a flush that sampled `_noteDirty`
  // right after the last character — a close, a `Ctrl+S`, a `pagehide` — saw a
  // clean card and wrote nothing, and the card was torn down before the
  // listener could fire. Every flush point goes through here first, which makes
  // the editor the source of truth and the listener only the thing that starts
  // the 800 ms clock.
  function syncFromEditor(el) {
    if (!el._noteEditor) return;
    let next;
    try {
      next = el._noteEditor.getMarkdown();
    } catch {
      return;
    }
    if (typeof next !== "string" || next === bodyOf(el._noteMarkdown)) return;
    el._noteMarkdown = dress(el, next);
    el._noteDirty = true;
    paintTitle(el);
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
  //
  // SERIALISED PER CARD, and that is load-bearing: `WBDaemon.observe` opens a
  // socket per call and orders nothing, and this card has four callers (the
  // debounce, `focusout`, `Ctrl+S`, `flushAll`). Two overlapping writes can
  // land oldest-last, and the newer reply has already cleared the dirty flag —
  // the card would then show text the file does not hold, with nothing
  // scheduled to fix it. `wb-console.js`'s `deskWrite` chain exists for exactly
  // this reason and this is the same shape.
  function flush(el) {
    clearTimeout(el._noteTimer);
    el._noteTimer = null;
    syncFromEditor(el);
    if (!el._noteDirty || el.classList.contains("missing")) return Promise.resolve();
    el._noteWrite = (el._noteWrite || Promise.resolve()).catch(() => {}).then(() => writeNow(el));
    return el._noteWrite;
  }

  function writeNow(el) {
    // Re-read EVERYTHING here: this runs at the tail of the chain, and the card
    // may have been saved, closed or emptied while it waited.
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
    // ONE naming per card, memoised synchronously: the probe below is a round
    // trip, and a second flush entering it would either write the same fresh
    // path twice or — once the first write has landed — step to `-2` and leave
    // the first file orphaned under a name nobody chose.
    if (el._noteNaming) return el._noteNaming;
    const dirField = el.querySelector(".note-dir");
    const dir = String(dirField?.value || DEFAULT_DIR)
      .trim()
      .replace(/^\/+|\/+$/g, "");
    const base = noteSlug(titleOf(markdown, "")) || stampName();
    el._noteNaming = firstFreeName(record, dir, base)
      .then((path) => {
        el._noteNaming = null;
        if (!path) {
          paintState(el, "could not name this note");
          return null;
        }
        patch(record.id, { path });
        paintPath(el, path);
        return path;
      })
      .catch((err) => {
        // A dropped socket mid-probe must not leave the card unable to ever
        // name itself: clear the memo and say why.
        el._noteNaming = null;
        paintState(el, String(err?.message || err));
        return null;
      });
    return el._noteNaming;
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
  // ---- the palette (ADR-0064 §8, amended 2026-09-22) ---------------------------

  // The ground and the ink, as two rows of swatches. The pair is the
  // operator's: the tones are desaturated by design, so a card that must READ
  // as yellow needs the solid fill, and a solid fill needs an ink chosen for
  // it. Rendered into the same popover so the two are picked together and the
  // card repaints under the pointer.
  let openPalette = null;
  function togglePalette(el) {
    const pop = el.querySelector(".note-palette");
    // A locked card is read-only, and the look lives in the document: offering
    // a swatch that cannot be written is offering a refusal.
    if (!pop || el._noteLocked) return;
    const wasOpen = !pop.hidden;
    closePalette();
    closeMenu();
    if (wasOpen) return;
    paintPalette(el);
    pop.hidden = false;
    openPalette = pop;
    document.addEventListener("pointerdown", closePaletteOutside, true);
  }
  function closePalette() {
    if (!openPalette) return;
    openPalette.hidden = true;
    openPalette = null;
    document.removeEventListener("pointerdown", closePaletteOutside, true);
  }
  function closePaletteOutside(ev) {
    if (openPalette && !openPalette.contains(ev.target) && !ev.target?.closest?.(".note-tone")) {
      closePalette();
    }
  }

  function buildPalette(el) {
    const pop = document.createElement("div");
    pop.className = "note-palette";
    pop.hidden = true;
    // The press must not reach the card's drag or the plane's accelerators,
    // and the swatch must not blur the editor before it repaints.
    pop.addEventListener("pointerdown", (ev) => ev.stopPropagation());
    pop.addEventListener("mousedown", (ev) => ev.preventDefault());
    const row = (label, cls, names, pick) => {
      const strip = document.createElement("div");
      strip.className = "note-swatches";
      const cap = document.createElement("span");
      cap.className = "note-swatch-label";
      cap.textContent = label;
      strip.append(cap);
      for (const name of names) {
        const b = document.createElement("button");
        b.className = cls;
        b.type = "button";
        b.dataset.name = name;
        b.title = name;
        b.setAttribute("aria-label", label + ": " + name);
        b.addEventListener("click", (ev) => {
          ev.stopPropagation();
          pick(el, name);
        });
        strip.append(b);
      }
      pop.append(strip);
    };
    row("Card", "note-swatch note-swatch-tone", TONES, setTone);
    row("Fill", "note-swatch note-swatch-fill", FILLS, setFill);
    row("Text", "note-swatch note-swatch-ink", INKS, setInk);
    return pop;
  }

  // Which swatch is the card's now — read off the card, so a palette opened on
  // a note whose file was hand-edited shows what the file says.
  function paintPalette(el) {
    const pop = el.querySelector(".note-palette");
    if (!pop) return;
    const have = { tone: el._noteTone, fill: el._noteFill, ink: el._noteInk };
    for (const [key, cls] of [
      ["tone", "note-swatch-tone"],
      ["fill", "note-swatch-fill"],
      ["ink", "note-swatch-ink"],
    ]) {
      for (const b of pop.querySelectorAll("." + cls)) {
        b.classList.toggle("is-on", b.dataset.name === have[key]);
      }
    }
  }

  // One restyle: the card repaints at once and the DOCUMENT is what carries
  // it, because the look is the file's (ADR-0064 §8) — close the card and
  // reopen it and the colours come back with the text.
  function restyle(el, next) {
    el._noteTone = toneOf(next.tone ?? el._noteTone);
    el._noteFill = fillOf(next.fill ?? el._noteFill);
    el._noteInk = inkOf(next.ink ?? el._noteInk);
    el.dataset.tone = el._noteTone;
    el.dataset.fill = el._noteFill;
    el.dataset.ink = el._noteInk;
    // Through `syncFromEditor` first: the operator may have typed a character
    // the change listener has not reported yet, and a restyle rewrites the
    // whole document.
    syncFromEditor(el);
    el._noteMarkdown = withStyle(el._noteMarkdown, {
      tone: el._noteTone,
      fill: el._noteFill,
      ink: el._noteInk,
    });
    paintPalette(el);
    markDirty(el);
  }
  const setTone = (el, tone) => restyle(el, { tone });
  const setFill = (el, fill) => restyle(el, { fill });
  const setInk = (el, ink) => restyle(el, { ink });

  // ---- the title, as a rename (ADR-0064 §4) -------------------------------------

  function beginTitle(el) {
    if (el._noteLocked) return;
    closePalette();
    closeMenu();
    const label = el.querySelector(".note-title");
    const field = el.querySelector(".note-title-edit");
    if (!label || !field) return;
    field.value = titleOf(el._noteMarkdown, "");
    label.hidden = true;
    field.hidden = false;
    field.focus();
    field.select();
  }
  function endTitle(el) {
    const label = el.querySelector(".note-title");
    const field = el.querySelector(".note-title-edit");
    if (!label || !field) return;
    field.hidden = true;
    label.hidden = false;
  }
  function commitTitle(el) {
    const field = el.querySelector(".note-title-edit");
    // `blur` is also what a teardown fires: a card removed with the field open
    // must not remount an editor onto a node that has left the document.
    if (!field || field.hidden || !el.isConnected) return;
    const next = field.value;
    endTitle(el);
    if (next === titleOf(el._noteMarkdown, "")) return;
    syncFromEditor(el);
    el._noteMarkdown = withTitle(el._noteMarkdown, next);
    // The editor holds the document, so a heading it did not write must be
    // handed back to it — a remount is the whole of `CrepeLean`'s surface for
    // "here is different text", and a retitle is rare enough to afford one.
    // The FILE keeps its name: `⋯ → Rename file…` is the other verb.
    mountEditor(el, el._noteMarkdown).then(() => markDirty(el));
    paintTitle(el);
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
      const href = a.getAttribute("href");
      const target = window.WBViewer?.linkTarget?.("", href) || popupLinkTarget(href);
      if (!target || target.kind === "fragment") {
        ev.preventDefault();
        return;
      }
      if (target.kind === "external") {
        // The SCHEME is an allowlist, and that is a security control, not
        // tidiness. The viewer's `linkTarget` answers "external" for ANY
        // scheme and leaves it to the browser — safe there only because the
        // viewer's markdown went through DOMPurify first (wb-viewer.js), which
        // drops a `javascript:` href. A note's markdown never meets that pass:
        // Crepe renders the link straight into the document, so a `.note` file
        // — repo bytes, which an agent may have written — could otherwise run
        // script on the daemon's own origin with one click.
        if (!isSafeScheme(href)) {
          ev.preventDefault();
          return;
        }
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

  // The three schemes a note may send the browser to. Everything else — and
  // that includes `javascript:`, `data:` and `vbscript:` — is inert.
  const SAFE_SCHEMES = ["http:", "https:", "mailto:"];
  function isSafeScheme(href) {
    const scheme = /^([a-z][a-z0-9+.\-]*):/i.exec(String(href || ""));
    // No scheme means a `/`-rooted path, which the browser resolves on this
    // origin: an ordinary navigation, not a foreign one.
    if (!scheme) return true;
    return SAFE_SCHEMES.includes(scheme[1].toLowerCase() + ":");
  }

  // The detached-fence popup loads no `wb-viewer.js`, so without this every
  // link in a note there would fall into the `!target` branch and silently do
  // nothing. A repo-relative link still cannot be opened from a popup that has
  // no explorer — that one stays inert, and says so by doing nothing.
  function popupLinkTarget(href) {
    if (!href) return null;
    if (href.startsWith("#")) return { kind: "fragment", fragment: href.slice(1) };
    if (/^[a-z][a-z0-9+.\-]*:/i.test(href) || href.startsWith("/")) return { kind: "external" };
    return null;
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
    // Nothing left to watch: a page that once showed a note must not keep a
    // 5 s timer and an observer alive for the document's life — a popup that
    // reattached its fence would carry both forever.
    if (!watched.size) {
      if (sweeper != null) clearInterval(sweeper);
      sweeper = null;
      observer?.disconnect();
      observer = null;
    }
  }

  // The ONE place a card is taken off the stage: the editor goes with it,
  // whether it had finished mounting or not.
  function tearDownCard(el) {
    el._noteGone = true;
    clearTimeout(el._noteTimer);
    el._noteTimer = null;
    untrackDormancy(el);
    const editor = el._noteEditor;
    el._noteEditor = null;
    try {
      editor?.destroy();
    } catch {}
    el.remove();
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
    // The cap refuses, it does not evict (see `saveNotes`).
    if (window.WBConsole?.atNoteCap?.()) {
      window.WBConsole.toast({ text: `at the ${window.WBConsole.NOTE_MAX}-note cap · close one first` });
      return null;
    }
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
        window.WBConsole?.fenceRecords?.() || [],
      ),
      locked: false,
      ts: Date.now(),
    };
    window.WBConsole.saveNotes(records.concat([record]));
    render();
    const el = cardEl(record.id);
    if (el) {
      window.WBConsole.focusWin(el);
      // "already in edit with the cursor placed" (ADR-0064 §9) — the editor
      // mounts a tick later, so the intent is left on the card and honoured by
      // `mountEditor`. Without it the first act after "New note" is a click
      // into a card that is already focused.
      el._noteFocusOnMount = true;
    }
    return record.id;
  }

  // Close the card, keep the file (ADR-0064 §11). The undo is what makes this
  // safe to do with one click on a document nobody asked to delete.
  function closeCard(id) {
    const record = recordOf(id);
    if (!record) return;
    const el = cardEl(id);
    // Whatever was typed lands before the card goes: a close is not a discard —
    // including the character typed a frame ago, which the editor knows about
    // before its listener does.
    if (el) syncFromEditor(el);
    // `.catch` is not decoration — the naming probe rejects on a socket the
    // daemon closed without a reply, and without it the close button would
    // silently do nothing at exactly the moment the operator is trying to
    // tidy up.
    const written = el && el._noteDirty ? flush(el).catch(() => {}) : Promise.resolve();
    written.then(() => {
      // Read the record AFTER the flush: closing a never-saved note performs
      // its first save, and the path the undo must restore is the one that
      // save just chose — the pre-flush snapshot aims at nothing.
      const saved = recordOf(id) || record;
      window.WBConsole.saveNotes((window.WBConsole.notes() || []).filter((n) => n.id !== id));
      render();
      window.WBConsole.toast({
        text: saved.path ? `note closed · ${saved.path} kept` : "note closed",
        action: "Undo",
        onAction: () => {
          window.WBConsole.saveNotes(
            (window.WBConsole.notes() || []).concat([{ ...saved, ts: Date.now() }]),
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

  // This document holds a FRAGMENT of the plane — one detached fence's members
  // — not the desk. Latched by `mountDetached`, because the popup reads the
  // whole desk (every document runs `reloadDesk`) while its detach registry is
  // deliberately inert: without this, the first `render()` a lock or a close
  // triggers there would build a card, an editor and a `note.read` for every
  // note on the shell's plane.
  let fragment = false;

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
      // In a fragment, an unknown record is not this window's to show.
      if (fragment && !nodes.has(record.id)) continue;
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
      tearDownCard(el);
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
    fragment = true;
    const el = buildCard(record);
    loadInto(el, record);
    paint(el, record, []);
    return el;
  }

  // ---- the file actions (ADR-0064 §11) ------------------------------------------

  // The `⋯` menu. One open at a time, closed by the next press anywhere else —
  // the card is a small surface and a menu left open over the text is in the
  // way of the thing it belongs to.
  let openMenu = null;
  function toggleMenu(el) {
    const menu = el.querySelector(".note-menu-card");
    if (!menu) return;
    const wasOpen = !menu.hidden;
    closeMenu();
    if (wasOpen) return;
    const record = recordOf(el.dataset.noteId);
    // Only a saved note in the PRIMARY tree has file actions: a worktree note
    // cannot be renamed or deleted yet (ADR-0064, amendment: `note.write` is
    // the only Write verb that crosses the worktree gate).
    const writable = !!record?.path && !record?.checkout;
    const missing = el.classList.contains("missing");
    for (const item of menu.querySelectorAll(".note-menu-item")) {
      // Re-aiming is the MISSING card's verb and the only one it has: renaming
      // and deleting need a file that is there.
      item.disabled = item.textContent.startsWith("Point") ? !record : !writable || missing;
    }
    menu.hidden = false;
    openMenu = menu;
    document.addEventListener("pointerdown", closeOnOutside, true);
  }
  function closeMenu() {
    if (!openMenu) return;
    openMenu.hidden = true;
    openMenu = null;
    document.removeEventListener("pointerdown", closeOnOutside, true);
  }
  function closeOnOutside(ev) {
    if (openMenu && !openMenu.contains(ev.target) && !ev.target?.closest?.(".note-more")) {
      closeMenu();
    }
  }

  // Rename the FILE and follow it with the record (ADR-0064 §4: a title change
  // never renames, so this is the only way a note's name moves). The daemon's
  // `file.rename` is what reaches it, through the denylist's one carve-out.
  function baseName(path) {
    return path.includes("/") ? path.slice(path.lastIndexOf("/") + 1) : path;
  }
  function dirName(path) {
    return path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
  }

  // Point a card at another file (ADR-0064 §11's missing-file state). The
  // record is re-aimed and the file is re-read; nothing on disk is touched,
  // because there is nothing there to touch — which is why this is a separate
  // verb from the rename beside it.
  function pointElsewhere(el) {
    const record = recordOf(el.dataset.noteId);
    if (!record) return;
    const field = el.querySelector(".note-rename");
    if (!field) return;
    el._notePointing = true;
    field.value = record.path || "";
    field.hidden = false;
    const path = el.querySelector(".note-path");
    if (path) path.hidden = true;
    field.focus();
    field.select();
  }

  function renameNote(el) {
    const record = recordOf(el.dataset.noteId);
    if (!record?.path || record.checkout) return;
    const field = el.querySelector(".note-rename");
    const path = el.querySelector(".note-path");
    if (!field) return;
    field.value = baseName(record.path);
    field.hidden = false;
    if (path) path.hidden = true;
    field.focus();
    field.select();
  }
  function endRename(el) {
    const field = el.querySelector(".note-rename");
    if (!field || field.hidden) return;
    field.hidden = true;
    el._notePointing = false;
    const path = el.querySelector(".note-path");
    if (path) path.hidden = false;
  }
  function commitRename(el) {
    const record = recordOf(el.dataset.noteId);
    const field = el.querySelector(".note-rename");
    const next = String(field?.value || "").trim();
    const pointing = !!el._notePointing;
    endRename(el);
    if (!record || !next) return;
    if (pointing) {
      // A whole rel path, not a file name: the card is being aimed somewhere
      // else in the checkout, and the read decides whether there is a note
      // there. Identity is `(repo, checkout, path)` (ADR-0064 §4) and this is
      // the one gesture that could break it: two cards over one file would be
      // two editors autosaving it, each overwriting the other with stale text
      // and neither told.
      const taken = (window.WBConsole?.notes?.() || []).find(
        (n) =>
          n.id !== record.id &&
          n.repo === record.repo &&
          (n.checkout ?? null) === (record.checkout ?? null) &&
          n.path === next,
      );
      if (taken) {
        paintState(el, "another card already holds that note");
        return;
      }
      patch(record.id, { path: next });
      paintPath(el, next);
      el.classList.remove("missing");
      loadInto(el, { ...record, path: next });
      return;
    }
    if (!record.path || next === baseName(record.path)) return;
    // Always a `.note`: the verbs refuse anything else, and a note renamed out
    // of its extension would be a file nothing can open (ADR-0064 §5).
    const name = next.endsWith(".note") ? next : next + ".note";
    const dir = dirName(record.path);
    const to = (dir ? dir + "/" : "") + name;
    window.WBDaemon.write("file.rename", { repo: record.repo, path: record.path, to })
      .then((reply) => {
        if (window.WBFail.isError(reply)) {
          paintState(el, window.WBFail.message(reply, "rename refused"));
          return;
        }
        patch(record.id, { path: to });
        paintPath(el, to);
        paintState(el, "renamed");
      })
      .catch((err) => paintState(el, String(err?.message || err)));
  }

  // Delete the file — a SEPARATE act from closing the card (§11), confirmed,
  // and it takes the card with it because there is nothing left to show.
  function deleteNote(el) {
    const record = recordOf(el.dataset.noteId);
    if (!record?.path || record.checkout) return;
    window.WBConsole.askConfirm({
      title: "Delete this note's file?",
      message: `${record.path} is deleted from the checkout. This cannot be undone.`,
      confirmLabel: "Delete",
      danger: true,
    }).then((ok) => {
      if (!ok) return;
      window.WBDaemon.write("file.delete", { repo: record.repo, path: record.path })
        .then((reply) => {
          if (window.WBFail.isError(reply)) {
            paintState(el, window.WBFail.message(reply, "delete refused"));
            return;
          }
          // The record goes without a toast: an undo that cannot put the file
          // back would be a lie.
          el._noteDirty = false;
          window.WBConsole.saveNotes(
            (window.WBConsole.notes() || []).filter((n) => n.id !== record.id),
          );
          render();
        })
        .catch((err) => paintState(el, String(err?.message || err)));
    });
  }

  // ---- opening from the explorer (ADR-0064 §11) ---------------------------------

  // A double-click on a `.note` in the tree: jump to the card if it is already
  // on the plane (identity is `(repo, checkout, path)`), else put one there.
  // The read happens in `loadInto`, so a file that is not a note lands as the
  // missing/refused state on a card the operator can close — never silently.
  function openFromExplorer({ repo, checkout, path, viewport, offset }) {
    if (!repo || !path) return null;
    const tree = checkout ?? null;
    const already = (window.WBConsole?.notes?.() || []).find(
      (n) => n.repo === repo && (n.checkout ?? null) === tree && n.path === path,
    );
    if (already) return window.WBNotes.jump(already.id);
    if (window.WBConsole?.atNoteCap?.()) {
      window.WBConsole.toast({ text: `at the ${window.WBConsole.NOTE_MAX}-note cap · close one first` });
      return null;
    }
    const records = window.WBConsole?.notes?.() || [];
    const record = {
      id: `note-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
      repo,
      checkout: tree,
      path,
      rect: spawnRect(
        viewport || { width: 0, height: 0 },
        offset || { left: 0, top: 0 },
        records.length,
        window.WBConsole?.fenceRecords?.() || [],
      ),
      locked: false,
      ts: Date.now(),
    };
    window.WBConsole.saveNotes(records.concat([record]));
    render();
    return window.WBNotes.jump(record.id);
  }

  // ---- the map (ADR-0064 §10) ---------------------------------------------------

  // The notes on the plane, in desk order: what the `Note` menu draws. The
  // title and the anchors come from the LIVE card (the text is the card's, not
  // the desk's), so a note edited since it was opened lists what it says now.
  // A card that is away in a detached fence is still listed — the row jumps
  // this window's viewport to where the fence is, which is where it will be
  // when it comes home.
  function list() {
    const fences = window.WBConsole?.fenceRecords?.() || [];
    return (window.WBConsole?.notes?.() || []).map((record) => {
      const el = cardEl(record.id);
      const markdown = el?._noteMarkdown || "";
      const fence = window.WBGeometry?.fenceOf?.(fences, record.rect || {});
      return {
        id: record.id,
        title: titleOf(markdown, "Untitled note"),
        tone: toneOf(el?._noteTone),
        path: record.path || "",
        fence: fence?.name || "",
        anchors: anchorsOf(markdown),
      };
    });
  }

  // Jump to a card and, when `index` names one, to the `index`-th `##` inside
  // it. The heading is found in the ProseMirror DOM by ORDINAL, not by text: a
  // note may hold two sections with the same name, and the anchor list is
  // built from the same document in the same order.
  function jump(id, index) {
    const el = window.WBConsole?.jumpToNote?.(id);
    if (!el || index == null) return el;
    const body = el.querySelector(".note-body");
    const heading = body?.querySelectorAll?.("h2")?.[index];
    if (!heading) return el;
    heading.scrollIntoView({ block: "start", behavior: reducedMotion() ? "auto" : "smooth" });
    // The ring goes on the CARD, not on the heading. MEASURED: a class added to
    // a node inside the editor is stripped within a frame — ProseMirror owns
    // that DOM and reconciles foreign attributes away — so the heading cannot
    // carry it. The scroll says WHERE; this says WHICH.
    el.classList.add("jumped");
    setTimeout(() => el.classList.remove("jumped"), 1200);
    return el;
  }

  function reducedMotion() {
    try {
      return !!window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches;
    } catch {
      return false;
    }
  }

  // Every card writes what it holds before the page goes. A best effort by
  // definition — the socket may not finish — but an 800 ms debounce loses a
  // sentence without it, and this is the same bargain `wb-console.js` takes
  // for the desk on `pagehide`.
  function flushAll() {
    const st = stage();
    if (!st) return;
    for (const el of st.querySelectorAll(".note-card")) {
      // `flush` re-reads the editor itself, so an unannounced last keystroke
      // is carried here too.
      flush(el);
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
    styleOf,
    withStyle,
    withTitle,
    fillOf,
    inkOf,
    lockedBy,
    spawnRect,
    noteDormancyDecision,
    TONES,
    FILLS,
    INKS,
    DEFAULT_TONE,
    DEFAULT_FILL,
    DEFAULT_INK,
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
    list,
    jump,
    openFromExplorer,
  };
})();
