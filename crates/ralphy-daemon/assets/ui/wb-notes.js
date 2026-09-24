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
  // The hand the note is written in (asked 2026-09-22). A CLOSED SET like
  // every other field of the look, and for a reason this one makes sharper: a
  // free `font-family` string would put a font the writer happens to have
  // installed into a file someone else opens, and the card would paint as a
  // fallback nobody chose. The names are ROLES; 01-base.css owns which stack
  // each is on this machine (13-notes.css).
  const FONTS = ["sans", "serif", "mono"];
  const DEFAULT_FONT = "sans";
  // The reading size, as a STEP and not a pixel count. `--reading-size` is the
  // plane's own (ADR-0035) and each step is a ratio of it, so a note keeps its
  // relation to the chrome around it instead of pinning a number that stops
  // agreeing with the rest of the workbench the moment that scale moves.
  const SIZES = ["xs", "s", "m", "l", "xl"];
  const DEFAULT_SIZE = "m";
  // What the palette calls each name of the sets above. The file keeps the
  // key; only the tooltip and the accessible name read this.
  const SWATCH_NAME = {
    ochre: "Ochre",
    sage: "Sage",
    rose: "Rose",
    slate: "Slate",
    plum: "Plum",
    sand: "Sand",
    wash: "Tint",
    solid: "Solid",
    default: "Theme",
    light: "Light",
    dark: "Dark",
    sans: "Sans serif",
    serif: "Serif",
    mono: "Monospace",
    xs: "Extra small",
    s: "Small",
    m: "Medium",
    l: "Large",
    xl: "Extra large",
  };
  // What a NEW note is dressed in, which is NOT the same question as what an
  // absent field means. The `DEFAULT_*` above are the READING fallback — a
  // file that names no `fill:` is a wash — and changing them to restyle new
  // notes would restyle every note already written, because the field they
  // omit is the one being redefined. So the operator's choice (2026-09-22)
  // lives here and is written out explicitly by `withHeader`.
  const NEW_NOTE_STYLE = { tone: "ochre", fill: "solid", ink: "dark" };
  // The default landing directory (ADR-0064 §4), mirrored from `note::DIR`.
  const DEFAULT_DIR = ".ralphy/notes";
  // Autosave: quiet for this long and the note is written (ADR-0064 §7). Short
  // enough that a closed tab loses a sentence at most, long enough that typing
  // a paragraph is one write and not forty.
  const SAVE_AFTER_MS = 800;

  // ---- pure folds (tested without a document) ---------------------------------

  // A note's title is the front matter's `title:` (ADR-0064 §4, amended
  // 2026-09-22: the name lives in the header, not in the body). The LEGACY
  // shape is still read — a `#` heading on the body's first line, which is
  // where every note written before the amendment carries its name — so an
  // existing file opens under the name it has always had. An untitled note is
  // called what the card calls it.
  function titleOf(markdown, fallback) {
    const named = titleFieldOf(markdown);
    if (named !== null) return named;
    const legacy = legacyTitleOf(bodyOf(markdown));
    if (legacy) return legacy.name;
    return fallback === undefined ? "Untitled note" : fallback;
  }

  // The body's leading `#` heading, when that is what the body opens with —
  // `{name, rest}`, or `null`. Deliberately NOT "the first `#` anywhere", the
  // old rule: with the title out of the document, a `#` further down is a
  // section the operator wrote and absorbing it into the header would delete
  // text nobody asked to move.
  function legacyTitleOf(body) {
    const lines = String(body || "").split("\n");
    let at = 0;
    while (at < lines.length && lines[at].trim() === "") at += 1;
    const m = /^#\s+(.+?)\s*$/.exec(lines[at] ?? "");
    if (!m) return null;
    // The blank line that separated the heading from the text goes with it.
    let after = at + 1;
    if ((lines[after] ?? "").trim() === "") after += 1;
    return { name: m[1], rest: lines.slice(after).join("\n") };
  }

  // When this note was last written, for the footer (ADR-0064 §7 as amended).
  // The TIME on the day it happened and the date as well on any other: a note
  // saved four minutes ago and one saved last Tuesday must not read alike, and
  // "19:42" alone says the wrong thing about the second. Local, because the
  // operator saved it where they are. `null` for a note no save has landed
  // for yet — a card is not going to claim a time it does not have.
  function savedLabel(at, now) {
    if (!at) return "";
    const then = new Date(at);
    const today = new Date(now ?? Date.now());
    const hh = String(then.getHours()).padStart(2, "0");
    const mm = String(then.getMinutes()).padStart(2, "0");
    const sameDay =
      then.getFullYear() === today.getFullYear() &&
      then.getMonth() === today.getMonth() &&
      then.getDate() === today.getDate();
    if (sameDay) return `Saved ${hh}:${mm}`;
    const dd = String(then.getDate()).padStart(2, "0");
    const mo = String(then.getMonth() + 1).padStart(2, "0");
    return `Saved ${dd}/${mo} ${hh}:${mm}`;
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
  function fontOf(name) {
    return FONTS.includes(name) ? name : DEFAULT_FONT;
  }
  function sizeOf(name) {
    return SIZES.includes(name) ? name : DEFAULT_SIZE;
  }

  // The look a card is WEARING, in the two places it has to be: the fields the
  // writers read, and the `data-*` the stylesheet selects on. One function
  // because the look grew from three fields to five — every site that set them
  // by hand was a place the next field would be forgotten, and a card wearing
  // four of five is a card whose file and paint disagree.
  function applyLook(el, style) {
    el._noteTone = toneOf(style?.tone);
    el._noteFill = fillOf(style?.fill);
    el._noteInk = inkOf(style?.ink);
    el._noteFont = fontOf(style?.font);
    el._noteSize = sizeOf(style?.size);
    el.dataset.tone = el._noteTone;
    el.dataset.fill = el._noteFill;
    el.dataset.ink = el._noteInk;
    el.dataset.font = el._noteFont;
    el.dataset.size = el._noteSize;
  }

  // What that card is wearing, as the shape every writer takes.
  function lookOf(el) {
    return {
      tone: el._noteTone,
      fill: el._noteFill,
      ink: el._noteInk,
      font: el._noteFont,
      size: el._noteSize,
    };
  }

  // A field on a card is NOT a credential. Measured (2026-09-22): renaming a
  // note raised the browser's "save your password?" prompt with the note's
  // title offered as the username — the password manager had classified the
  // title box as the login form's user field, because every input the page
  // leaves unowned by a `<form>` is grouped into one synthetic form with the
  // login password beside it. The attributes below are the documented way out,
  // and the two `data-*` ones say the same thing to 1Password and LastPass,
  // which read their own.
  function noCredential(input) {
    input.type = "text";
    input.autocomplete = "off";
    input.setAttribute("autocorrect", "off");
    input.setAttribute("autocapitalize", "off");
    input.setAttribute("data-1p-ignore", "");
    input.setAttribute("data-lpignore", "true");
    input.setAttribute("data-form-type", "other");
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

  // Does this note OPEN VEILED (ADR-0064 §8 as amended)? `hidden: true` is the
  // note saying it is not for whoever happens to be looking at the screen.
  // Deliberately NOT part of `styleOf`: the look is how the card is painted,
  // and this decides whether there is anything painted at all.
  function veiledOf(markdown) {
    return fieldOf(markdown, "hidden", ["true", "false"]) === "true";
  }

  // The `title:` field — free text, so it cannot go through `fieldOf`'s closed
  // set. `null` when the block has none, which is what tells `titleOf` to look
  // for the legacy heading. Written as a double-quoted YAML scalar and read as
  // one, because a title says "Sprint 12: what is left" often enough that a
  // bare value would be invalid YAML to anyone else's parser.
  function titleFieldOf(markdown) {
    const split = splitFrontMatter(markdown);
    if (!split) return null;
    for (const line of split.inner.split("\n")) {
      const m = /^\s*title:\s*(.*?)\s*$/.exec(line.replace(/\r$/, ""));
      if (!m) continue;
      const raw = m[1];
      if (!raw.startsWith('"')) return raw;
      return raw
        .slice(1, raw.endsWith('"') && raw.length > 1 ? -1 : undefined)
        .replace(/\\(["\\])/g, "$1");
    }
    return null;
  }
  function titleYaml(name) {
    return '"' + String(name).replace(/[\\"]/g, "\\$&") + '"';
  }

  // The whole look of the card, defaulted: the ground's tone, how much of it
  // the ground takes, and the ink over it (ADR-0064 §8, amended 2026-09-22).
  function styleOf(markdown) {
    return {
      tone: toneOf(colorOf(markdown) || DEFAULT_TONE),
      fill: fillOf(fieldOf(markdown, "fill", FILLS) || DEFAULT_FILL),
      ink: inkOf(fieldOf(markdown, "ink", INKS) || DEFAULT_INK),
      font: fontOf(fieldOf(markdown, "font", FONTS) || DEFAULT_FONT),
      size: sizeOf(fieldOf(markdown, "size", SIZES) || DEFAULT_SIZE),
    };
  }

  // `body` under the one front-matter block this shell writes. ONE shape, so
  // an autosave never rewrites the header for no reason — and the three
  // defaults are OMITTED, so a note nobody restyled and nobody named carries
  // the same single `color:` line it always did. `title` leads because it is
  // the document's name; `fill`/`ink` trail because they are refinements of
  // the colour above them.
  function withHeader(body, title, style, veiled) {
    const tone = toneOf(style?.tone);
    const fill = fillOf(style?.fill);
    const ink = inkOf(style?.ink);
    const font = fontOf(style?.font);
    const size = sizeOf(style?.size);
    const name = String(title || "").replace(/[\r\n]+/g, " ").trim();
    const lines = [];
    if (name) lines.push(`title: ${titleYaml(name)}`);
    lines.push(`color: ${tone}`);
    if (fill !== DEFAULT_FILL) lines.push(`fill: ${fill}`);
    if (ink !== DEFAULT_INK) lines.push(`ink: ${ink}`);
    if (font !== DEFAULT_FONT) lines.push(`font: ${font}`);
    if (size !== DEFAULT_SIZE) lines.push(`size: ${size}`);
    // Last, and only when true: `hidden: false` is what every note in the
    // world already is, and writing it would put a line in every file to say
    // nothing.
    if (veiled) lines.push("hidden: true");
    return `---\n${lines.join("\n")}\n---\n${body}`;
  }

  // Restyling keeps the name, whatever the caller happens to hold: the two
  // fields live in one block and a writer that knows about only one of them
  // would drop the other every time it ran.
  function withStyle(markdown, style) {
    // The FIELD, never `titleOf`: promoting a legacy heading here would half
    // migrate a note — the name in the header and the heading still in the
    // body — on a gesture that was about colour. `withTitle` is the one
    // migration door.
    return withHeader(bodyOf(markdown), titleFieldOf(markdown) ?? "", style, veiledOf(markdown));
  }

  // The veil, as a WRITE — the ONE door that changes it, which is what lets
  // every other writer carry it without knowing what it is for.
  function withVeil(markdown, veiled) {
    return withHeader(bodyOf(markdown), titleFieldOf(markdown) ?? "", styleOf(markdown), !!veiled);
  }

  // The visible name, as a WRITE (ADR-0064 §4 as amended 2026-09-22: the title
  // is a front-matter field, so retitling never touches the body and never
  // renames the file — the latter is what `⋯ → Rename file…` is for).
  //
  // This is also the ONE place a legacy note migrates: the heading the old
  // rule called the title is lifted out of the body and into the header, so
  // the name stops being shown twice. It happens on a retitle and nowhere else
  // — opening a note rewrites nothing.
  function withTitle(markdown, title) {
    const name = String(title || "").replace(/[\r\n]+/g, " ").trim();
    const body = bodyOf(markdown);
    const legacy = legacyTitleOf(body);
    return withHeader(legacy ? legacy.rest : body, name, styleOf(markdown), veiledOf(markdown));
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

  // Paint the lock. A locked card cannot be MOVED or RESIZED, and that is the
  // whole of it (the operator's words, 2026-09-22: "o cadeado no notes não é
  // impedir de editar é impedir de mover"). The note is still written, renamed,
  // recoloured and hidden — a pin through the card, not a read-only file. The
  // JS guards in `makeDraggable`/`startResize` are the truth for the gestures;
  // this is the glyph.
  function applyLock(el, locked) {
    el._noteLocked = !!locked;
    el.classList.toggle("locked", !!locked);
    const btn = el.querySelector(".note-lock");
    if (btn) {
      // The console's own two glyphs, verbatim (`wb-console.js`'s `applyLock`):
      // one lock on the plane, not one per surface.
      btn.innerHTML = locked ? '<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>';
      btn.title = locked ? "Unlock this note" : "Lock this note in place";
    }
  }

  // The card's chrome for one record. Mirrors `buildFence`'s construction
  // order, including its hit-test rule.
  function buildCard(record) {
    const el = document.createElement("div");
    el.className = "note-card";
    el.dataset.noteId = record.id;
    // A card with NO FILE YET is a new note and is born in the operator's
    // colours; one with a path is dressed from what it is about to read, and
    // starting it anywhere but the reading default would flash a colour the
    // file does not hold for the tick the read takes.
    const born = record.path
      ? { tone: DEFAULT_TONE, fill: DEFAULT_FILL, ink: DEFAULT_INK }
      : NEW_NOTE_STYLE;
    applyLook(el, born);
    // Everything the card knows that is not in the desk record: the text, what
    // has been written, and what is in flight. Deliberately NOT desk state —
    // the file is the note (ADR-0064 §2).
    el._noteMarkdown = "";
    el._noteDirty = false;
    el._noteInFlight = false;
    el._noteTimer = null;
    el._noteSavedAt = null;
    // The SESSION half of the veil: the file says whether a note opens veiled,
    // this says whether it has been shown since. Never persisted — that is the
    // whole point of the pair.
    el._noteRevealed = false;

    const head = document.createElement("div");
    head.className = "note-head";
    const grab = document.createElement("span");
    grab.className = "note-grab";
    grab.title = "Move this note";
    // Bootstrap Icons, like every other control on the plane: the card's
    // chrome was the one surface drawing its controls as text characters, and
    // a braille-dots grip beside a console's `bi-grip-vertical` reads as a
    // different application. The console's titlebar is the reference
    // (`buildChrome`), and a test pins the characters back OUT.
    grab.innerHTML = '<i class="bi bi-grip-vertical"></i>';
    const title = document.createElement("span");
    title.className = "note-title";
    title.title = "Rename this note";
    // Renaming IS editing the first `#` heading (ADR-0064 §4) — the title has
    // no storage of its own, so this field writes the document. An INPUT and
    // not `contenteditable`: the head is a drag handle, and a caret inside a
    // handle is two gestures on one pixel.
    const titleEdit = document.createElement("input");
    titleEdit.className = "note-title-edit";
    titleEdit.setAttribute("aria-label", "Note title");
    noCredential(titleEdit);
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
    tone.title = "Change the colors and the font";
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
    // The card's own index (ADR-0064 §10). The `Note` menu already maps every
    // card's `##`; this is the same fold read from ONE card, in its head,
    // where an operator scrolling a long note is already looking. It does not
    // move the card — the note is already in front of them.
    const index = document.createElement("button");
    index.className = "note-index";
    index.type = "button";
    index.title = "Go to a heading in this note";
    index.hidden = true;
    index.innerHTML = '<i class="bi bi-list-ul"></i>';
    index.addEventListener("click", (ev) => {
      ev.stopPropagation();
      toggleIndex(el);
    });
    // The eye (ADR-0064 §8 as amended). Shown ONLY on a note that carries
    // `hidden: true`, because on any other card it would be a control for a
    // state that card is not in — the same rule `Use another file…` follows.
    // It shows and re-hides for THIS SESSION and writes nothing: the mark is
    // what the file carries, and looking at a note is not a change to it.
    const veil = document.createElement("button");
    veil.className = "note-veil";
    veil.type = "button";
    veil.addEventListener("click", (ev) => {
      ev.stopPropagation();
      // ALWAYS THERE, and the act depends on the card's state: a note nobody
      // has hidden is hidden by this press (the file write the `⋯` menu also
      // offers); a hidden one is shown and put away again, which writes
      // nothing. The operator asked for the eye to be available on every card
      // (2026-09-22) — a control that appears only once the state is already
      // reached is not a door into it.
      if (!veiledOf(el._noteMarkdown)) setMarked(el, true);
      else toggleReveal(el);
    });
    // THE FILE'S OWN CONTROL, and it lives with the file: a gear in the
    // footer's left corner, beside the path it acts on (asked 2026-09-22).
    // It was a `⋯` in the head, which put the delete two pixels from the
    // close button and read as "more of the head's controls" — but nothing in
    // this menu is about the card. The footer already names the file; this is
    // the button that changes it.
    const more = document.createElement("button");
    more.className = "note-more";
    more.type = "button";
    more.title = "File and Markdown help";
    more.innerHTML = '<i class="bi bi-gear"></i>';
    more.addEventListener("click", (ev) => {
      ev.stopPropagation();
      toggleMenu(el);
    });
    const close = document.createElement("button");
    close.className = "note-close";
    close.type = "button";
    close.title = "Close this note. The file is kept.";
    close.innerHTML = '<i class="bi bi-x-lg"></i>';
    close.addEventListener("click", () => closeCard(el.dataset.noteId));
    tools.append(tone, index, veil, lock, close);

    // The file actions (ADR-0064 §11), in a menu rather than in the head: they
    // act on the FILE, not on the card, and two more glyphs beside the close
    // button is where a slip becomes a deletion.
    const menu = document.createElement("div");
    menu.className = "note-menu-card";
    menu.hidden = true;
    const renameItem = document.createElement("button");
    renameItem.className = "note-menu-item note-menu-file";
    renameItem.type = "button";
    renameItem.textContent = "Rename file…";
    renameItem.addEventListener("click", () => {
      closeMenu();
      renameNote(el);
    });
    const drop = document.createElement("button");
    drop.className = "note-menu-item note-menu-file danger";
    drop.type = "button";
    drop.textContent = "Delete file…";
    drop.addEventListener("click", () => {
      closeMenu();
      deleteNote(el);
    });
    // ONLY the missing card's verb, and hidden otherwise (ADR-0064 §11: a
    // missing card's tools shrink to *remove* and *point elsewhere*). Offered
    // beside `Rename file…` on a healthy note it read as a second, cryptic
    // rename — "I don't know what this point… is" (operator, 2026-09-22) —
    // for a state that card is not in.
    const point = document.createElement("button");
    point.className = "note-menu-item note-menu-point";
    point.type = "button";
    point.textContent = "Use another file…";
    point.title = "The file of this note is gone. Choose another file.";
    point.hidden = true;
    point.addEventListener("click", () => {
      closeMenu();
      pointElsewhere(el);
    });
    // UNMARKING ONLY, and hidden on a note that is not marked — the rule
    // `Use another file…` follows, for the same reason. The operator asked for
    // `Hide this note` to go (2026-09-22): the eye in the head hides any card,
    // so the entry was a second door to a place that already had one. The
    // OTHER direction is not a duplicate of anything — the eye can mark and
    // reveal but never unmark, so deleting the item outright would strand a
    // veiled note with no way back — and it is what stays here.
    const mark = document.createElement("button");
    mark.className = "note-menu-item note-menu-mark";
    mark.type = "button";
    mark.textContent = "Stop hiding this note";
    mark.hidden = true;
    mark.addEventListener("click", () => {
      closeMenu();
      setMarked(el, false);
    });
    // Not a file action, which is why it is under a rule: the two above write
    // the disk and this one only says how to write the note (the sheet itself
    // is `markdownHelp`).
    const help = document.createElement("button");
    help.className = "note-menu-item note-menu-help";
    help.type = "button";
    help.textContent = "Markdown help";
    help.addEventListener("click", () => {
      closeMenu();
      markdownHelp();
    });
    menu.append(renameItem, point, drop, mark, help);

    // The index's own dropdown, filled on each open from the LIVE document —
    // the headings change with every keystroke and a list built once would
    // name sections that are no longer there.
    const anchors = document.createElement("div");
    anchors.className = "note-menu-card note-anchors";
    anchors.hidden = true;

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
    dir.setAttribute("aria-label", "Folder for this note");
    noCredential(dir);
    dir.value = DEFAULT_DIR;
    dir.title = "Where this note is saved";
    // The plane's accelerators must not fire on a directory being typed.
    dir.addEventListener("keydown", (e) => e.stopPropagation());
    // The rename field (ADR-0064 §11), in the footer beside the path it
    // replaces while an edit is open. An INPUT and not `window.prompt`: the
    // native dialog is dismissed by default in an automated browser, which is
    // exactly the objection `wb-console.js` records against `window.confirm`.
    const rename = document.createElement("input");
    rename.className = "note-rename";
    rename.setAttribute("aria-label", "New file name for this note");
    noCredential(rename);
    rename.hidden = true;
    rename.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.key === "Enter") commitRename(el);
      else if (ev.key === "Escape") endRename(el);
    });
    rename.addEventListener("blur", () => endRename(el));
    const state = document.createElement("span");
    state.className = "note-state";
    foot.append(more, path, dir, rename, state);

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
    el.append(...handles, head, body, foot, tools, menu, anchors, palette);
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
    //
    // ANYTHING THAT IS NOT THE EDITOR, not "is the body": measured again
    // 2026-09-22 in a browser, the blank space under the last line belongs to
    // Crepe's own `.milkdown` wrapper, which sits between the two — so the
    // `ev.target !== body` this guard used to be let every one of those
    // presses through to the default, which BLURRED the editor. The keystrokes
    // after it went to `document.body` and were lost with no sign on the card.
    body.addEventListener("mousedown", (ev) => {
      const dom = el._noteEditor?.dom?.();
      if (!dom) return;
      if (dom === ev.target || dom.contains(ev.target)) return;
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
    // The NAME comes off the card's document, not off `body`: the editor never
    // sees the header, so `withStyle` here would read the title out of the
    // body it was handed and find nothing — and rewrite the field away on the
    // first keystroke. The FIELD and not `titleOf`, for the mirror reason: a
    // legacy note's heading is still in the body being typed into, and copying
    // it up would print the name twice until the next retitle.
    const title = titleFieldOf(el._noteMarkdown) ?? "";
    return withHeader(
      body,
      title,
      { tone: el._noteTone, fill: el._noteFill, ink: el._noteInk },
      veiledOf(el._noteMarkdown),
    );
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
    applyLook(el, style);
    body.textContent = "";
    // A VEILED CARD MOUNTS NO EDITOR (ADR-0064 §8 as amended). Not a blur and
    // not `display: none`: the text is never put into the document at all, and
    // the markdown held here is cut down to its header, so the tab does not
    // carry the body either. Revealing RE-READS the file — which is also what
    // waking from dormancy does, and for the same reason.
    if (veiledOf(markdown) && !el._noteRevealed) {
      el._noteMarkdown = withVeil(withHeader("", titleFieldOf(markdown) ?? "", style), true);
      body.append(veilPanel());
      paintVeil(el);
      paintTitle(el);
      paintState(el, "");
      return Promise.resolve(null);
    }
    paintVeil(el);
    // Marked here and cleared on teardown: `create()` resolves a tick or two
    // later, and a card closed, detached or evicted in between would otherwise
    // leave a live ProseMirror view with its listeners on a detached node —
    // the exact leak dormancy exists to prevent.
    el._noteGone = false;
    return window.CrepeLean.create({
      root: body,
      value: bodyOf(markdown),
      // Never read-only from the lock: a locked card is pinned, not frozen.
      readonly: false,
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
        // The footer says when this note last landed, from the moment it
        // opens — not only after the card has saved it once itself.
        if (!el._noteDirty) paintState(el, "");
        if (el._noteFocusOnMount) {
          el._noteFocusOnMount = false;
          try {
            editor?.dom()?.focus();
          } catch {}
        }
        return editor;
      })
      .catch((err) => {
        paintState(el, "Could not start the editor: " + String(err?.message || err));
        return null;
      });
  }

  // Read the note's file into the card. A missing file is a STATE, not a
  // disappearance: the card stays, says so, and — §11 as amended 2026-09-22 —
  // keeps an editor, because the way back from that state is the operator
  // writing in it.
  function loadInto(el, record) {
    if (!record.path) return mountEditor(el, dress(el, ""));
    return window.WBDaemon.observe(
      "note.read",
      window.WBDaemon.withCheckout({ repo: record.repo, path: record.path }, record.checkout),
    )
      .then((reply) => {
        if (window.WBFail.isError(reply)) {
          const reason = window.WBFail.message(reply, "Could not read the file.");
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
          // The reason in the footer, beside the path that footer already
          // shows — and the whole of it on hover, because `.note-state` is a
          // narrow box and "not found" alone is the half that matters.
          paintMissing(el, reason, `${record.path} — ${reason}`);
          // An editor over what the card still holds — the unsaved text if
          // there is any, and an empty document if the read is all this card
          // ever had. Typing in it makes the card dirty, and a dirty card
          // writes its file back. Without this the missing state is a dead
          // end: a red line where the note was and no way to put it back.
          if (el._noteEditor) return null;
          // The line is repainted AFTER the mount: mounting an editor ends by
          // clearing the footer, and the card would go translucent while
          // saying nothing about why.
          return mountEditor(el, el._noteMarkdown || dress(el, "")).then((mounted) => {
            paintMissing(el, reason, `${record.path} — ${reason}`);
            return mounted;
          });
        }
        el.classList.remove("missing");
        // After a reload the card remembers no save of its own, and this is
        // the only place the answer exists (`note.read` carries it).
        el._noteSavedAt = Number(reply.modified) || null;
        return mountEditor(el, reply.markdown || "");
      })
      .catch((err) => {
        paintMissing(el, String(err?.message || err));
        return null;
      });
  }

  // The card says its file is gone — in the FOOTER, and in the dashed border
  // the `missing` class draws. The body is left alone: a read that fails must
  // not take the text with it, and emptying it is the one irreversible thing
  // this state can do. Measured 2026-09-22 from the operator's screenshot — a
  // card in a fence went translucent and blank while pointing at a path no
  // file was ever written to, and what it blanked had never reached the disk.
  function paintMissing(el, reason, full) {
    el.classList.add("missing");
    paintState(el, reason);
    const state = el.querySelector(".note-state");
    if (state) state.title = full || reason;
  }

  // The head, repainted from the document: both the name and whether there is
  // an index are folds of `_noteMarkdown`, so every caller that has one has
  // the other.
  function paintTitle(el) {
    const title = el.querySelector(".note-title");
    if (title) title.textContent = titleOf(el._noteMarkdown, "Untitled note");
    paintIndex(el);
  }

  // `text` is the momentary word — "…", "Saved", a refusal. EMPTY means idle,
  // and idle is where the last-save stamp lives: the footer is the one place
  // that can answer "when did this land" without opening the file's
  // properties (asked 2026-09-22).
  function paintState(el, text) {
    const state = el.querySelector(".note-state");
    if (!state) return;
    if (text) {
      state.textContent = text;
      state.title = "";
      return;
    }
    state.textContent = savedLabel(el._noteSavedAt);
    state.title = el._noteSavedAt ? new Date(el._noteSavedAt).toLocaleString() : "";
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
    // DIRTY IS THE WHOLE TEST, and `missing` deliberately is not (ADR-0064 §11
    // as amended 2026-09-22, the operator's call): a card whose file is gone
    // but which holds an edit the operator typed writes it back and recreates
    // the file. That is not "behind their back" — it is the text they are
    // looking at. A missing card that is CLEAN holds only a stale copy of a
    // file somebody deleted, and `!dirty` already refuses it.
    if (!el._noteDirty) return Promise.resolve();
    el._noteWrite = (el._noteWrite || Promise.resolve()).catch(() => {}).then(() => writeNow(el));
    return el._noteWrite;
  }

  function writeNow(el) {
    // Re-read EVERYTHING here: this runs at the tail of the chain, and the card
    // may have been saved, closed or emptied while it waited.
    if (!el._noteDirty) return Promise.resolve();
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
            paintState(el, window.WBFail.failed(reply, "Could not save: the daemon gave no reason."));
            return;
          }
          el.classList.remove("danger");
          // The file exists NOW — including the case where it had gone and
          // this write is what brought it back, which is why the missing state
          // is cleared here and not only by a read.
          el.classList.remove("missing");
          // The record may carry its path: this is the moment a claim becomes
          // a name (see `namePath`).
          if (!record.path) {
            patch(record.id, { path });
            el._noteClaim = null;
          }
          // Only for the bytes that landed: a keystroke during the write leaves
          // the card dirty, and the next debounce carries it.
          if (el._noteMarkdown === markdown) el._noteDirty = false;
          // The stamp is OUR clock and not the file's: `note.write` replies
          // with no mtime, and a second round trip to ask for one would be a
          // read per keystroke-pause. The two agree to within the write.
          el._noteSavedAt = Date.now();
          paintState(el, "Saved");
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
    // A name this card CLAIMED but has not written yet. The desk record is
    // patched only once bytes have landed (`writeNow`), so without this a
    // second flush would probe again and — the first write having created the
    // file — step to `-2`, orphaning it under a name nobody chose.
    if (el._noteClaim) return Promise.resolve(el._noteClaim);
    // ONE naming per card, memoised synchronously: the probe below is a round
    // trip, and a second flush entering it would race the first.
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
          paintState(el, "Could not name this note.");
          return null;
        }
        // CLAIMED, not recorded. A path in the desk record is a promise that
        // a file is there: patch it before the write and a write that never
        // lands leaves the card pointing at a file nobody created — which is
        // exactly the translucent "not found" card the operator found on
        // 2026-09-22, holding text that had never reached the disk.
        el._noteClaim = path;
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
      field.title = path || "This note is not saved yet";
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
    // A VEILED card is refused: it holds only its header, so writing the
    // document from here would save that header OVER the note. A LOCKED one is
    // not — the lock pins the card, it does not freeze the note.
    if (!pop || veiledNow(el)) return;
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
    // `text` turns the strip from swatches into CHIPS — a colour can be shown
    // as itself, a font and a size cannot, so those two rows name what they
    // offer and the font chips are drawn IN the font they name.
    const row = (label, cls, names, pick, text) => {
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
        b.title = SWATCH_NAME[name] || name;
        if (text) b.textContent = text(name);
        b.setAttribute("aria-label", label + ": " + (SWATCH_NAME[name] || name));
        b.addEventListener("click", (ev) => {
          ev.stopPropagation();
          pick(el, name);
        });
        strip.append(b);
      }
      pop.append(strip);
    };
    row("Color", "note-swatch note-swatch-tone", TONES, setTone);
    row("Fill", "note-swatch note-swatch-fill", FILLS, setFill);
    row("Text", "note-swatch note-swatch-ink", INKS, setInk);
    // `Aa` in each face, and the step names for the size: the chip is a sample
    // of what pressing it does, which is the only honest label a font has.
    row("Font", "note-swatch note-swatch-font", FONTS, setFont, () => "Aa");
    row("Size", "note-swatch note-swatch-size", SIZES, setSize, (n) => n.toUpperCase());
    return pop;
  }

  // Which swatch is the card's now — read off the card, so a palette opened on
  // a note whose file was hand-edited shows what the file says.
  function paintPalette(el) {
    const pop = el.querySelector(".note-palette");
    if (!pop) return;
    const have = lookOf(el);
    for (const [key, cls] of [
      ["tone", "note-swatch-tone"],
      ["fill", "note-swatch-fill"],
      ["ink", "note-swatch-ink"],
      ["font", "note-swatch-font"],
      ["size", "note-swatch-size"],
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
    applyLook(el, { ...lookOf(el), ...next });
    // Through `syncFromEditor` first: the operator may have typed a character
    // the change listener has not reported yet, and a restyle rewrites the
    // whole document.
    syncFromEditor(el);
    el._noteMarkdown = withStyle(el._noteMarkdown, lookOf(el));
    // A mermaid fence is an SVG with its colours written into it as inline
    // fills (`entry.js` seeds them from this card), so the cascade cannot
    // reach it — without this, restyling a note left every diagram in it
    // wearing the tone the card used to be.
    try {
      el._noteEditor?.redrawDiagrams?.();
    } catch {}
    paintPalette(el);
    markDirty(el);
  }
  const setTone = (el, tone) => restyle(el, { tone });
  const setFill = (el, fill) => restyle(el, { fill });
  const setInk = (el, ink) => restyle(el, { ink });
  const setFont = (el, font) => restyle(el, { font });
  const setSize = (el, size) => restyle(el, { size });

  // ---- the title, as a rename (ADR-0064 §4) -------------------------------------

  function beginTitle(el) {
    // Veiled for `togglePalette`'s reason: the card is holding a header, not
    // the note, and a retitle writes the whole document. A locked card renames
    // like any other.
    if (veiledNow(el)) return;
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
    // A veiled card has already given its editor back and is already showing
    // the one line it is allowed to show; putting it to sleep would replace
    // that with the "asleep" line and lose what the card is saying.
    if (veiledNow(el)) return;
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
      window.WBConsole.toast({ text: `You can have at most ${window.WBConsole.NOTE_MAX} notes. Close one first.` });
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
        // The path IS the sentence: "note closed · <path> kept" said the same
        // thing twice, and `kept` was reassuring nobody about the file the
        // `✕` never touches (asked 2026-09-22).
        text: saved.path ? `Note ${saved.path} closed` : "Note closed",
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

  // The head's dropdowns — the `⋯` file menu and the `☰` index. ONE open at a
  // time across the whole plane, closed by the next press anywhere else: the
  // card is a small surface and a menu left open over the text is in the way
  // of the thing it belongs to. They share this opener because they share that
  // rule; two independent close-on-outside listeners left one hanging when the
  // other opened.
  let openMenu = null;
  let openMenuTrigger = "";
  function openDrop(el, popSelector, triggerSelector, fill) {
    const pop = el.querySelector(popSelector);
    if (!pop) return;
    const wasOpen = !pop.hidden;
    closeMenu();
    closePalette();
    if (wasOpen) return;
    if (fill && fill(pop) === false) return;
    pop.hidden = false;
    openMenu = pop;
    openMenuTrigger = triggerSelector;
    document.addEventListener("pointerdown", closeOnOutside, true);
  }
  function toggleMenu(el) {
    openDrop(el, ".note-menu-card:not(.note-anchors)", ".note-more", (menu) => {
      const record = recordOf(el.dataset.noteId);
      // Only a saved note in the PRIMARY tree has file actions: a worktree note
      // cannot be renamed or deleted yet (ADR-0064, amendment: `note.write` is
      // the only Write verb that crosses the worktree gate).
      const writable = !!record?.path && !record?.checkout;
      const missing = el.classList.contains("missing");
      // Renaming and deleting need a file that is there.
      for (const item of menu.querySelectorAll(".note-menu-file")) {
        item.disabled = !writable || missing;
      }
      // Only on a note that IS marked: hiding is the eye's, and this is the
      // one door out of it.
      const mark = menu.querySelector(".note-menu-mark");
      if (mark) {
        mark.hidden = !veiledOf(el._noteMarkdown);
        mark.disabled = missing;
      }
      // Re-aiming is the MISSING card's verb and the only one it has, so it is
      // not shown at all until the card is in that state.
      const point = menu.querySelector(".note-menu-point");
      if (point) {
        point.hidden = !missing;
        point.disabled = !record;
      }
    });
  }

  // The index (ADR-0064 §10), rebuilt on every open from the live document.
  function toggleIndex(el) {
    openDrop(el, ".note-anchors", ".note-index", (pop) => {
      syncFromEditor(el);
      const found = anchorsOf(el._noteMarkdown);
      pop.textContent = "";
      if (!found.length) return false;
      for (const a of found) {
        const row = document.createElement("button");
        row.className = "note-menu-item";
        row.type = "button";
        row.textContent = a.text;
        row.title = a.text;
        row.addEventListener("click", () => {
          closeMenu();
          scrollToAnchor(el, a.index);
        });
        pop.append(row);
      }
      return true;
    });
  }

  // Show the index button only when there is an index. A card whose note has
  // no `##` would otherwise offer a control that opens onto nothing.
  function paintIndex(el) {
    const btn = el.querySelector(".note-index");
    if (!btn) return;
    btn.hidden = anchorsOf(el._noteMarkdown).length === 0;
  }

  // Scroll this card's body to its `index`-th `##`. By ORDINAL and not by
  // text, for `jump`'s reason: a note may hold two sections with one name.
  //
  // The BODY is scrolled, and nothing else. `heading.scrollIntoView()` walks
  // every scrollable ancestor and moves each one — so picking a section also
  // panned the plane under the card, which is the operator's report: the
  // canvas moved when only the note should have. The offset is computed
  // against the body's own box, which is the one box that must move.
  function scrollToAnchor(el, index) {
    const body = el.querySelector(".note-body");
    const heading = body?.querySelectorAll?.("h2")?.[index];
    if (!body || !heading) return;
    const top =
      heading.getBoundingClientRect().top - body.getBoundingClientRect().top + body.scrollTop;
    body.scrollTo({ top: Math.max(0, top), behavior: reducedMotion() ? "auto" : "smooth" });
  }

  function closeMenu() {
    if (!openMenu) return;
    openMenu.hidden = true;
    openMenu = null;
    openMenuTrigger = "";
    document.removeEventListener("pointerdown", closeOnOutside, true);
  }
  function closeOnOutside(ev) {
    if (openMenu && !openMenu.contains(ev.target) && !ev.target?.closest?.(openMenuTrigger)) {
      closeMenu();
    }
  }

  // ---- the veil (ADR-0064 §8 as amended) ----------------------------------------

  // Is this card showing nothing RIGHT NOW? The mark is the file's and the
  // reveal is the session's; a card is veiled when it carries the first and
  // has not been given the second.
  function veiledNow(el) {
    return veiledOf(el._noteMarkdown) && !el._noteRevealed;
  }

  // What the body holds instead of an editor. Says what it is and what opens
  // it — a card that is simply blank reads as one that failed to load.
  // A VEILED CARD SHOWS ONE GLYPH AND NO SENTENCE (asked 2026-09-22). The line
  // it replaces named the control that opens it — "the eye above shows it" —
  // which is instruction for a gesture the operator has already learnt, printed
  // on every hidden note forever. The point of the veil is that nothing about
  // the note is on screen; a caption is the one thing still talking.
  //
  // The glyph keeps its `title`, so the answer is still a hover away.
  function veilPanel() {
    const p = document.createElement("p");
    p.className = "note-veiled";
    p.title = "This note is hidden. Click the eye button to show it.";
    p.innerHTML = '<i class="bi bi-eye-slash"></i>';
    return p;
  }

  // The eye: on EVERY card, and its glyph is the ACT it offers, not the state
  // it is in. Three states, two acts — hide this note (a write), show it, put
  // it away again (both session-only). Unmarking stays in the `⋯` menu, which
  // is where the other writes to the file live.
  function paintVeil(el) {
    const btn = el.querySelector(".note-veil");
    if (!btn) return;
    const marked = veiledOf(el._noteMarkdown);
    const shown = marked && !!el._noteRevealed;
    const hides = !marked || shown;
    btn.innerHTML = hides ? '<i class="bi bi-eye-slash"></i>' : '<i class="bi bi-eye"></i>';
    btn.title = !marked ? "Hide this note" : shown ? "Hide this note again" : "Show this note";
    el.classList.toggle("veiled", veiledNow(el));
  }

  // Show it, or put it away again. Writes NOTHING: the mark stays whatever the
  // file says, so a note shown once is veiled again on the next open.
  function toggleReveal(el) {
    if (!veiledOf(el._noteMarkdown)) return;
    const record = recordOf(el.dataset.noteId);
    if (!record) return;
    if (el._noteRevealed) {
      // Away first, THEN the teardown: `loadInto` reads the file again, and
      // the card must not be left holding a body it is no longer showing.
      el._noteRevealed = false;
      loadInto(el, record);
      return;
    }
    el._noteRevealed = true;
    loadInto(el, record);
  }

  // Mark the note as one that opens veiled, or stop. A DOCUMENT write, so it
  // goes through the same autosave every other edit does — and marking puts
  // the card away in the same gesture, because marking a note you are looking
  // at and leaving it on screen is half an act.
  function setMarked(el, marked) {
    // A VEILED CARD HOLDS ONLY ITS HEADER — `mountEditor` cuts the body out
    // rather than put it in a document nobody is looking at — so unmarking
    // straight from `_noteMarkdown` would write that bare header OVER the note
    // and the text would be gone. MEASURED against the code path, not guessed:
    // `withVeil` rebuilds from `bodyOf`, and the body it would find is `""`.
    // So the file is read back FIRST and the unmark rides the load out, the
    // same door `toggleReveal` opens.
    if (!marked && veiledNow(el)) {
      const record = recordOf(el.dataset.noteId);
      if (!record) return;
      el._noteRevealed = true;
      loadInto(el, record).then(() => setMarked(el, false));
      return;
    }
    // Through the editor first: a character typed a frame ago is not in
    // `_noteMarkdown` yet, and this rewrites the whole document.
    syncFromEditor(el);
    el._noteMarkdown = withVeil(el._noteMarkdown, marked);
    el._noteRevealed = !marked;
    markDirty(el);
    paintVeil(el);
    // The write lands first; only then does the body go, or the flush would
    // find an editor that had already been taken away from under it.
    flush(el).then(() => {
      const record = recordOf(el.dataset.noteId);
      if (record) loadInto(el, record);
    });
  }

  // ---- the cheat sheet ----------------------------------------------------------

  // WHAT TO TYPE. The editor is hybrid WYSIWYG (ADR-0064 §6) — the markup
  // dissolves the moment it is recognised — which is exactly why the marks
  // themselves are not discoverable: there is nothing left on screen to read
  // them off. The `/` menu answers the same question for BLOCKS and nothing
  // answered it for marks.
  //
  // The flavour is CommonMark + GFM, because that is what the editor's two
  // presets parse; every row below was typed into the real bundle and its
  // markdown read back (2026-09-22), so nothing here is a guess at upstream's
  // rules. Three of them are this bundle's own additions, because upstream had
  // no rule at all: `[ ] ` on a plain line, `[text](url)` (commonmark ships an
  // input rule for an IMAGE and none for a link), and the `@@path` shorthand.
  const MARKDOWN_HELP = [
    ["**bold**", "Bold"],
    ["*italic*", "Italic"],
    ["`code`", "Inline code"],
    ["~~struck~~", "Strikethrough"],
    ["[text](url)", "A link"],
    ["@@path/to/file", "A link to that file"],
    ["#", "Large heading"],
    ["##", "Heading. It is also listed in the heading index."],
    ["-", "A bullet"],
    ["1.", "A numbered item"],
    ["[ ]", "A task. [x] marks it as done."],
    [">", "A quote"],
    ["|3x3|", "A 3×3 table"],
    ["---", "A horizontal line"],
    ["```", "A code block"],
    ["```mermaid", "A diagram. Click it to edit."],
    ["/", "The block menu"],
    ["Shift+Enter", "A line break inside the block"],
  ];
  // The trailing SPACE is what fires a block mark, and it cannot be seen in a
  // chip — so the table drops it and this line carries it instead.
  const MARKDOWN_HELP_NOTE = "Type a space after a mark to apply it.";

  // Borrowed from `wb-console.js`'s `askConfirm`: the shell's modal CLASSES
  // over DOM this module builds itself, so the card keeps working in the
  // detached-fence popup, which has no Alpine and no modal markup.
  function markdownHelp() {
    const scrim = document.createElement("div");
    scrim.className = "modal-scrim wb-note-help";
    const modal = document.createElement("div");
    modal.className = "modal note-help-modal";
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-label", "Markdown in a note");
    const head = document.createElement("div");
    head.className = "modal-head";
    head.innerHTML = '<i class="bi bi-markdown"></i>';
    const heading = document.createElement("span");
    heading.className = "modal-title";
    heading.textContent = "Markdown in a note";
    const sub = document.createElement("span");
    sub.className = "modal-sub";
    sub.textContent = "CommonMark + GFM";
    head.append(heading, sub);
    const table = document.createElement("table");
    table.className = "note-help-table";
    for (const [type, gets] of MARKDOWN_HELP) {
      const tr = document.createElement("tr");
      const td1 = document.createElement("td");
      const code = document.createElement("code");
      code.textContent = type;
      td1.append(code);
      const td2 = document.createElement("td");
      td2.textContent = gets;
      tr.append(td1, td2);
      table.append(tr);
    }
    const note = document.createElement("p");
    note.className = "note-help-note";
    note.textContent = MARKDOWN_HELP_NOTE;
    const foot = document.createElement("div");
    foot.className = "modal-foot";
    const ok = document.createElement("button");
    ok.className = "btn accent";
    ok.type = "button";
    ok.textContent = "Close";
    foot.append(ok);
    modal.append(head, table, note, foot);
    scrim.append(modal);
    document.body.append(scrim);
    ok.focus();
    const done = () => {
      document.removeEventListener("keydown", onKey, true);
      scrim.remove();
    };
    const onKey = (ev) => {
      if (ev.key === "Escape" || (ev.key === "Enter" && document.activeElement === ok)) {
        ev.stopPropagation();
        done();
      }
    };
    // CAPTURE, for `askConfirm`'s reason: the plane's accelerators and a
    // console's terminal both take Escape before a bubbling listener would.
    document.addEventListener("keydown", onKey, true);
    ok.addEventListener("click", done);
    scrim.addEventListener("mousedown", (ev) => {
      if (ev.target === scrim) done();
    });
    return scrim;
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
        paintState(el, "Another note already uses that file.");
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
          paintState(el, window.WBFail.failed(reply, "Could not rename: the daemon gave no reason."));
          return;
        }
        patch(record.id, { path: to });
        paintPath(el, to);
        paintState(el, "Renamed");
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
            paintState(el, window.WBFail.failed(reply, "Could not delete: the daemon gave no reason."));
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
      window.WBConsole.toast({ text: `You can have at most ${window.WBConsole.NOTE_MAX} notes. Close one first.` });
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
    // Through the same fold the index uses: `jumpToNote` has already put the
    // plane where the card is, and a second scroll that reaches the plane
    // would move it again, away from what it just chose.
    scrollToAnchor(el, index);
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
    savedLabel,
    noteSlug,
    stampName,
    toneOf,
    bodyOf,
    colorOf,
    styleOf,
    withStyle,
    withTitle,
    titleFieldOf,
    veiledOf,
    withVeil,
    fillOf,
    inkOf,
    fontOf,
    sizeOf,
    lockedBy,
    spawnRect,
    noteDormancyDecision,
    TONES,
    FILLS,
    INKS,
    FONTS,
    SIZES,
    DEFAULT_TONE,
    DEFAULT_FILL,
    DEFAULT_INK,
    DEFAULT_FONT,
    DEFAULT_SIZE,
    DEFAULT_DIR,
    NEW_NOTE_STYLE,
    MARKDOWN_HELP,
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
    markdownHelp,
    openFromExplorer,
  };
})();
