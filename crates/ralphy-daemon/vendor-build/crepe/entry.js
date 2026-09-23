// The LEAN Crepe bundle the note card edits in (ADR-0064 §6).
//
// Crepe's own default entry (`@milkdown/crepe`) pulls every feature it ships,
// including CodeMirror (a second editor engine beside the vendored Monaco),
// KaTeX, the image-block uploader and a top bar the card has no room for. The
// `CrepeBuilder` takes features one at a time, which is what makes a 716 KB
// bundle out of a 2.5 MB one — measured in the spike
// (docs/spike-note-editor-2026-09-22.md).
//
// The SEVEN features, and why each is here:
//   block-edit    — the slash menu; the Notion feel itself. Its other half,
//                   the `+`/`⠿` block handle, is configured OFF below: it
//                   needs a gutter a 320 px card does not have.
//   cursor        — the drop/gap cursor, without which block drags aim blind
//   link-tooltip  — editing a link without seeing its markup
//   list-item     — task lists (`- [ ]`), the thing notes are made of
//   placeholder   — an empty card says what to do
//   table         — GFM tables render as tables (ADR-0064 §3)
//   toolbar       — the selection toolbar (bold/italic/link)
// And the five left out: code-mirror (Monaco is this workbench's ONE editor
// engine, #308), latex, image-block, top-bar, ai.
import { CrepeBuilder } from '@milkdown/crepe/builder';
import { blockEdit } from '@milkdown/crepe/feature/block-edit';
import { cursor } from '@milkdown/crepe/feature/cursor';
import { linkTooltip } from '@milkdown/crepe/feature/link-tooltip';
import { listItem } from '@milkdown/crepe/feature/list-item';
import { placeholder } from '@milkdown/crepe/feature/placeholder';
import { table } from '@milkdown/crepe/feature/table';
import { toolbar } from '@milkdown/crepe/feature/toolbar';
import {
  editorViewCtx,
  nodeViewCtx,
  remarkStringifyOptionsCtx,
} from '@milkdown/kit/core';
import { listenerCtx } from '@milkdown/kit/plugin/listener';
import { bulletListSchema, linkSchema, listItemSchema } from '@milkdown/kit/preset/commonmark';
import { InputRule } from '@milkdown/kit/prose/inputrules';
import { findWrapping } from '@milkdown/kit/prose/transform';
import { $inputRule } from '@milkdown/kit/utils';

// `[ ] ` ON A PLAIN LINE makes a task, the way `- ` makes a bullet.
//
// GFM's own rule (`preset-gfm`, `wrapInTaskListInputRule`) only sets the
// `checked` attribute and REQUIRES a list item to already be there — read and
// then measured 2026-09-22: `- [ ] ` works, and `[ ] ` alone is not just
// inert, it is escaped into the document as the literal text `\[ ]`. Since a
// note is mostly checklists and the brackets are what an operator reaches for
// (asked, same day), this rule does the wrapping half as well. It stands DOWN
// inside a list, where upstream's rule is the right one.
const taskFromBareBrackets = $inputRule((ctx) =>
  new InputRule(/^\[(?<checked>\s|x)\]\s$/, (state, match, start, end) => {
    const listItem = listItemSchema.type(ctx);
    const checked = match.groups?.checked === 'x';
    const $from = state.doc.resolve(start);
    for (let d = $from.depth; d > 0; d--) {
      const node = $from.node(d);
      if (node.type !== listItem) continue;
      // Upstream's rule owns a PLAIN item — it turns it into a task — and
      // refuses one that is already a task (`checked != null`), which left
      // `[x] ` typed into a task item as literal escaped text. Retyping the
      // box is how an operator ticks one from the keyboard, so that half is
      // taken here.
      if (node.attrs.checked == null) return null;
      return state.tr
        .delete(start, end)
        .setNodeMarkup($from.before(d), undefined, { ...node.attrs, checked });
    }
    const tr = state.tr.delete(start, end);
    const range = tr.doc.resolve(tr.mapping.map(start)).blockRange();
    const wrapping = range && findWrapping(range, bulletListSchema.type(ctx));
    if (!wrapping) return null;
    tr.wrap(range, wrapping);
    // The attribute goes on the ITEM the wrap just made, found by walking up
    // from where the text was: `findWrapping` decides for itself how many
    // nodes it takes to fit a paragraph into a list, so its depth is not ours
    // to assume.
    const $at = tr.doc.resolve(tr.mapping.map(start));
    for (let d = $at.depth; d > 0; d--) {
      if ($at.node(d).type === listItem) {
        tr.setNodeMarkup($at.before(d), undefined, { ...$at.node(d).attrs, checked });
        return tr;
      }
    }
    return null;
  }),
);

// `[text](url)` TYPED IS A LINK. Commonmark's preset ships an input rule for
// an image and none for a link — measured 2026-09-22: typing the markdown for
// one left it as text, and the autosave escaped it into the file as
// `\[here]\(path)`. Until now the only doors were the selection toolbar and
// the `/` menu, neither of which is what someone writing markdown reaches for.
//
// The URL is `[^()\s]+` deliberately: a link whose target holds a bracket or a
// space is the case this rule hands back to the toolbar rather than guess at.
const linkFromMarkdown = $inputRule((ctx) =>
  new InputRule(/\[([^\]]+)]\(([^()\s]+)\)$/, (state, match, start, end) => {
    const [, text, href] = match;
    const mark = linkSchema.type(ctx).create({ href });
    // STORED MARKS cleared, and that is the whole difference between a link
    // and a link that eats the rest of the sentence: the mark is active at
    // the caret after the replacement, so the next characters typed join it —
    // measured 2026-09-22, `[here](p) e pronto` linked "here e pronto".
    return state.tr
      .replaceWith(start, end, state.schema.text(text, [mark]))
      .removeStoredMark(mark.type);
  }),
);

// `@@<path> ` IS THE SAME LINK, written once. A note about the repo is mostly
// paths, and `[src/main.rs](src/main.rs)` is the same string typed twice — so
// the shorthand expands to it, and ADR-0064 §12's resolution (relative to the
// checkout root, opened through the viewer) does the rest.
//
// The SPACE that closes it is not swallowed into the link: a mark that
// extended over the following space would carry into whatever is typed next.
const linkFromPathShorthand = $inputRule((ctx) =>
  new InputRule(/@@([^\s@]+)\s$/, (state, match, start, end) => {
    const href = match[1];
    const mark = linkSchema.type(ctx).create({ href });
    return state.tr
      .replaceWith(start, end, [state.schema.text(href, [mark]), state.schema.text(' ')])
      .removeStoredMark(mark.type);
  }),
);

// One editor over one element. The shell (`wb-notes.js`) holds the instance and
// never reaches past this surface, so swapping the engine is a change to this
// file and its build — not to the card.
export function create({ root, value, readonly, placeholder: hint, onChange }) {
  const builder = new CrepeBuilder({ root, defaultValue: value ?? '' });
  // MEASURED, and recorded so it is not re-attempted: the slash menu mounts
  // inside the editor's own element (~480px tall) and is therefore clipped by a
  // small card's scroll box. `blockEdit`'s `root` options are floating-ui
  // BOUNDARIES, not portals — passing `document.body` moves nothing. The card's
  // answer is its size: it opens at 320×260 and resizes, and every block the
  // menu offers is also reachable by typing it (`# `, `- [ ] `, `|`), which is
  // the path this editor was chosen for.
  builder
    .addFeature(cursor)
    .addFeature(listItem)
    .addFeature(linkTooltip)
    .addFeature(table)
    .addFeature(toolbar)
    .addFeature(blockEdit, {
      // Six heading levels are a page's outline. A card renders h4–h6 within a
      // tenth of an em of body text, so they are noise in a menu that must fit
      // inside the note — still typeable as `#### `, like every other block.
      //
      // The `+`/`⠿` BLOCK HANDLE is hidden too, and in CSS
      // (`styles/13-notes.css`) rather than here: `blockHandle.shouldShow` is
      // in Crepe's config type but `@milkdown/plugin-block` never reads it —
      // measured 2026-09-22, the handle still showed with it set to `false`.
      textGroup: { h4: null, h5: null, h6: null },
    })
    .addFeature(placeholder, { text: hint ?? 'Write a note…' });
  builder.editor.use([taskFromBareBrackets, linkFromMarkdown, linkFromPathShorthand]);

  builder.editor.config((ctx) => {
    // A ```mermaid fence DRAWS (ADR-0064 §15). A node view and not a
    // post-render pass: ProseMirror owns this DOM and reconciles anything
    // written into it away within a frame (measured, §10's ring). Every other
    // code block keeps the plain editable shape.
    //
    // Registered in `nodeViewCtx` and NOT as `editorViewOptionsCtx.nodeViews`,
    // which is where this started: Milkdown builds the view as
    // `new EditorView(el, { nodeViews: fromEntries(nodeViewCtx), ...options })`
    // — the spread puts OUR object last, so one node view passed that way
    // replaces every node view the features registered. Measured in a browser
    // (2026-09-22): a bullet list drew no bullet and a task list no checkbox,
    // because `list-item`'s node view had been dropped on the floor by the
    // mermaid one.
    //
    // The view asks the BUILDER whether the editor is read-only, because
    // `setReadonly` flips the view's `editable` option — which gates user
    // input, not a programmatic `dispatch`. Without this a locked card
    // (ADR-0064 §8) could still be rewritten through the popover, and the
    // edit would autosave.
    ctx.update(nodeViewCtx, (prev) => [
      ...prev.filter(([name]) => name !== 'code_block'),
      [
        'code_block',
        (node, view, getPos) => codeBlockView(node, view, getPos, () => builder.readonly),
      ],
    ]);
    // The markdown a save writes back must be the markdown the operator would
    // have typed: `-` for bullets (remark's default is `*`, which rewrites
    // every list on the first autosave and makes a diff out of nothing).
    ctx.set(remarkStringifyOptionsCtx, {
      ...ctx.get(remarkStringifyOptionsCtx),
      bullet: '-',
    });
  });

  if (onChange) {
    builder.on((listener) => {
      listener.markdownUpdated((_ctx, markdown) => onChange(markdown));
    });
  }

  return builder.create().then(() => {
    if (readonly) builder.setReadonly(true);
    return {
      // Everything the card needs, and nothing that would let it reach into
      // Milkdown's context and grow a second integration surface.
      getMarkdown: () => builder.getMarkdown(),
      setReadonly: (value) => builder.setReadonly(!!value),
      destroy: () => builder.destroy(),
      // The ProseMirror DOM, for the `##` anchor scroll (ADR-0064 §10).
      dom: () => {
        try {
          return builder.editor.ctx.get(editorViewCtx).dom;
        } catch {
          return null;
        }
      },
      // A DIAGRAM'S COLOURS ARE BAKED IN AT RENDER TIME — mermaid writes them
      // into the SVG as inline fills, not as CSS the card could cascade over.
      // So a card restyled after a fence was drawn kept the old palette until
      // something else made ProseMirror rebuild the node view. The card calls
      // this from `restyle`; each host kept the source it was drawn from, so
      // the redraw needs nothing from the document.
      redrawDiagrams: () => {
        const root = (() => {
          try {
            return builder.editor.ctx.get(editorViewCtx).dom;
          } catch {
            return null;
          }
        })();
        if (!root) return;
        for (const host of root.querySelectorAll('.note-mermaid-figure')) {
          if (typeof host._mermaidSource === 'string') drawMermaid(host, host._mermaidSource);
        }
      },
    };
  });
}

// ---- the mermaid node view (ADR-0064 §15) ------------------------------------

// The page's own `mermaid` and `DOMPurify` — the same two the file viewer
// draws with (`wb-viewer.js`), so a diagram looks identical in a note and in a
// README, and the SVG meets the same sanitizer. Absent (a shell that loaded
// neither), a mermaid fence stays a code block and nothing throws.
let mermaidReady = false;

// HOW MERMAID CHOOSES ITS COLOURS, and why a built-in theme cannot work on a
// card. Mermaid derives its whole palette from three or four seeds by
// lightening, darkening and INVERTING them — `primaryTextColor` is literally
// the inverse of `primaryColor` unless you say otherwise. It never looks at
// the page it is drawn on. So the built-in dark theme this replaces was
// contrasting against its own idea of a canvas and landing that slab on a
// card the operator had painted ochre. (The name of that theme is deliberately
// not written here: a Rust test pins it OUT as the assignment, and prose
// quoting it would red the gate — the card's glyph pin learned the same way.)
//
// The seeds are taken from the CARD instead, off the two properties that
// already resolve the note's look: `background-color` is `--note-ground` and
// `color` is `--note-ink`. Everything below is a mix of those two, so a
// diagram contrasts with the note it is in whatever tone, fill and ink that
// note is wearing — including a card restyled after the diagram was drawn,
// because the node view redraws from the live computed style.
// TWO computed forms, because the card produces both. A tone is a hex and
// resolves to `rgb(176, 138, 74)`; a WASH is a `color-mix()`, and Chrome
// resolves that to `color(srgb 0.23 0.21 0.2)` — 0..1 floats in a different
// function. Reading only the first left every washed card falling back to
// mermaid's own palette, which is the bug this whole path exists to avoid.
function rgbOf(value) {
  const text = String(value || '');
  const srgb = /color\(\s*srgb\s+([^)]+)\)/.exec(text);
  if (srgb) {
    const parts = srgb[1].split(/[ ,/]+/).filter(Boolean).map(Number).slice(0, 3);
    if (parts.length === 3 && parts.every((n) => !Number.isNaN(n))) {
      return parts.map((n) => Math.max(0, Math.min(255, Math.round(n * 255))));
    }
    return null;
  }
  const m = /rgba?\(([^)]+)\)/.exec(text);
  if (!m) return null;
  const parts = m[1].split(/[ ,/]+/).filter(Boolean).map(Number);
  return parts.length >= 3 && parts.every((n) => !Number.isNaN(n)) ? parts.slice(0, 3) : null;
}
function blend(a, b, t) {
  return a.map((v, i) => Math.round(v + (b[i] - v) * t));
}
function css(c) {
  return 'rgb(' + c[0] + ', ' + c[1] + ', ' + c[2] + ')';
}
function mix(a, b, t) {
  return css(blend(a, b, t));
}
// A SURFACE ON THE GROUND, made the way `.note-head` makes its own: a SHADE of
// the ground, not a mix of the ground with the ink. This is the lesson that
// file already records — "darkened against the ground rather than tinted over
// it: a solid card has the tone everywhere, and a head mixed from the tone
// would vanish into it" — and it is exactly what the first cut of this palette
// got wrong. Mixing a node toward the ink at a tenth left the diagram
// camouflaged into the card (seen 2026-09-22, an ochre note whose boxes were
// barely there).
//
// A node is a SURFACE THE LABEL IS PRINTED ON, so it moves AWAY FROM THE INK —
// not away from the ground, which is what the first two cuts of this tried.
// Shading it against the ground made boxes that read as holes punched in the
// note; lifting it always made a light card with a light ink paler still
// (both seen 2026-09-22). Away from the ink is the one direction that is right
// on every combination the palette allows, because the label is the thing that
// has to be readable and the outline carries the shape either way.
//
// Over a DARK ink the node becomes a page: half toward white, which on an
// ochre card is the kraft-and-cream a printed diagram has. Over a LIGHT ink it
// goes down instead, and by less — a fifth is enough to seat the shape without
// turning it into a slab of some other colour. Rec. 709 luminance, the
// coefficients the browser's own contrast maths uses.
function luminance(c) {
  return (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) / 255;
}
function lift(ground, ink, strong) {
  const dark = luminance(ink) < 0.5;
  const amount = (dark ? 0.5 : 0.22) * (strong === undefined ? 1 : strong);
  return mix(ground, dark ? [255, 255, 255] : [0, 0, 0], amount);
}
// A per-diagram `%%{init}%%` directive rather than a second `initialize()`.
// Two cards render at once and `initialize` is GLOBAL — the second call would
// repaint the first card's diagram in the second card's colours, and the race
// is invisible until two notes are open side by side. A directive travels with
// the source it is rendered from. It goes FIRST so a directive the operator
// wrote themselves is applied after it and wins. `securityLevel` is on
// mermaid's own `secure` list and cannot be reached this way, so `strict`
// holds whatever a note's bytes say.
function themeDirective(host) {
  const card = host.closest ? host.closest('.note-card') : null;
  const style = window.getComputedStyle(card || host);
  const ground = rgbOf(style.backgroundColor);
  const ink = rgbOf(style.color);
  if (!ground || !ink) return '';
  const vars = {
    // Left as the card's ground and NOT `transparent`: mermaid mixes this one
    // into other values, and a keyword it cannot parse takes the palette down
    // with it. The drawing is transparent because the host paints nothing
    // (13-notes.css) — the SVG lays down no canvas rect of its own.
    background: css(ground),
    // A node is a SURFACE LYING ON THE NOTE — see `lift`. The three grades
    // are the same surface at three heights, so a nested diagram still reads
    // as one drawing rather than three palettes.
    primaryColor: lift(ground, ink),
    secondaryColor: lift(ground, ink, 1.4),
    tertiaryColor: lift(ground, ink, 0.55),
    // The labels are the note's own text, at full strength. Said explicitly
    // because the default is `invert(primaryColor)`, which on a mid-tone
    // ground lands somewhere neither the card nor the operator asked for.
    primaryTextColor: css(ink),
    secondaryTextColor: css(ink),
    tertiaryTextColor: css(ink),
    textColor: css(ink),
    // THE DIAGRAM IS DRAWN IN THE NOTE'S PEN. Outlines and arrows carry the
    // whole structure — which box is a box, which diamond is a decision, what
    // points at what — so they are the ink itself, barely held back. The first
    // cut had them at 45 %, and on a solid card the shapes dissolved into the
    // ground they sat on.
    primaryBorderColor: mix(ground, ink, 0.85),
    secondaryBorderColor: mix(ground, ink, 0.85),
    tertiaryBorderColor: mix(ground, ink, 0.85),
    lineColor: mix(ground, ink, 0.8),
    // The little box behind a `|Yes|` on an edge: the GROUND, so the label sits
    // on the card rather than on a chip of some other colour.
    edgeLabelBackground: css(ground),
    nodeBorder: mix(ground, ink, 0.85),
    mainBkg: lift(ground, ink),
    clusterBkg: lift(ground, ink, 0.4),
    clusterBorder: mix(ground, ink, 0.6),
    titleColor: css(ink),
    // The note's own hand and size (ADR-0064 §8 amendment): a diagram set in
    // the plane's default beside prose the operator put in a serif reads as a
    // picture pasted in from somewhere else.
    fontFamily: style.fontFamily,
  };
  return '%%{init: ' + JSON.stringify({ theme: 'base', themeVariables: vars }) + '}%%\n';
}

function drawMermaid(host, source, tries) {
  // NOT ON THE STAGE YET. ProseMirror builds a node view and inserts its `dom`
  // AFTER the constructor returns, so the first draw of every diagram runs on
  // a detached node — where `closest('.note-card')` finds nothing and
  // `getComputedStyle` answers with empty strings. MEASURED 2026-09-22: every
  // fence rendered in mermaid's stock palette and only a later redraw (an
  // edit, a restyle) picked up the card. The draw waits a frame for the node
  // to be placed, and gives up after a handful — a node view can be built and
  // discarded without ever being inserted, and that must not spin forever.
  const left = tries === undefined ? 8 : tries;
  if (!host.isConnected && left > 0) {
    window.requestAnimationFrame(() => drawMermaid(host, source, left - 1));
    return;
  }
  // What this host was drawn FROM, so `redrawDiagrams` can repaint it in the
  // card's new colours without walking the document for the fence again.
  host._mermaidSource = source;
  // An EMPTY fence is not a broken diagram: it is the one the operator has
  // just created and has not written yet. Mermaid answers it with "no diagram
  // type detected", which reads as a failure on a fence that has never been
  // given a chance.
  if (!String(source || '').trim()) {
    host.classList.remove('note-mermaid-error');
    host.classList.add('note-mermaid-empty');
    host.textContent = 'Empty diagram — click to write one';
    return;
  }
  host.classList.remove('note-mermaid-empty');
  if (!window.mermaid) {
    host.textContent = source;
    return;
  }
  if (!mermaidReady) {
    // `strict`, not `loose`, and the reason is `wb-viewer.js`'s verbatim: a
    // diagram's source is repo bytes, and `loose` skips mermaid's own sanitize
    // pass, turning a `click A "javascript:…"` directive into a live link.
    // Initialised HERE rather than borrowed from the viewer because this
    // bundle also runs in the detached-fence popup, which loads no viewer.
    // `htmlLabels: false` is LOAD-BEARING, not a style: mermaid's default label
    // is HTML inside a `<foreignObject>`, and DOMPurify 3.4 dropped that tag
    // from its SVG allowlist (an mXSS vector) — so every label was removed on
    // the way in and the diagram arrived as unlabelled boxes. MEASURED against
    // 3.4.12. Plain `<text>` labels survive the sanitizer untouched.
    window.mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      // `base` is the only theme whose variables are meant to be replaced —
      // the built-ins compute over their own seeds and would fight the card's.
      // The per-diagram directive below is what actually carries the colours.
      theme: 'base',
      htmlLabels: false,
      flowchart: { htmlLabels: false },
    });
    mermaidReady = true;
  }
  const id = 'note-mermaid-' + Math.random().toString(36).slice(2, 10);
  window.mermaid
    .render(id, themeDirective(host) + source)
    .then(({ svg }) => {
      host.classList.remove('note-mermaid-error');
      host.innerHTML = window.DOMPurify
        ? window.DOMPurify.sanitize(svg, {
            USE_PROFILES: { svg: true, svgFilters: true, html: true },
          })
        : '';
      if (!window.DOMPurify) host.textContent = source;
    })
    .catch((err) => {
      // Mermaid draws its OWN "syntax error" bomb, and it draws it into
      // `document.body` — measured 2026-09-22: a half-typed fence put a
      // 200 px cartoon at the bottom-left of the workbench, outside every
      // card, where nothing on the plane could close it. It is identified by
      // the render id, so it is removed by it.
      for (const stray of [document.getElementById(id), document.getElementById('d' + id)]) {
        stray?.remove();
      }
      // The source, verbatim, in a box that says it failed: a diagram that
      // does not parse must not become an empty hole where the text was.
      host.classList.add('note-mermaid-error');
      const gap = String.fromCharCode(10, 10);
      host.textContent = 'mermaid: ' + (err?.message || err) + gap + source;
    });
}

// One node view for EVERY code block, because ProseMirror's `nodeViews` is
// keyed by node type and cannot be conditional: a mermaid fence renders, and
// anything else gets the plain `<pre><code>` it would have had.
function codeBlockView(node, view, getPos, isReadonly) {
  if (node.attrs.language !== 'mermaid') {
    const pre = document.createElement('pre');
    const code = document.createElement('code');
    pre.append(code);
    return { dom: pre, contentDOM: code };
  }

  const dom = document.createElement('div');
  dom.className = 'note-mermaid';
  dom.title = 'click to edit this diagram';
  const figure = document.createElement('div');
  figure.className = 'note-mermaid-figure';
  dom.append(figure);
  drawMermaid(figure, node.textContent);

  // The popover: a textarea over the diagram, live-previewing. `Escape`
  // cancels, `Ctrl+Enter` applies — and applying is ONE transaction replacing
  // the fence's text, so the editor's own change listener carries it into the
  // autosave like any other edit.
  let pop = null;
  const closePop = () => {
    pop?.remove();
    pop = null;
  };
  const openPop = () => {
    if (pop || isReadonly?.()) return;
    pop = document.createElement('div');
    pop.className = 'note-mermaid-pop';
    const area = document.createElement('textarea');
    area.className = 'note-mermaid-source';
    area.value = node.textContent;
    const preview = document.createElement('div');
    preview.className = 'note-mermaid-preview';
    const hint = document.createElement('div');
    hint.className = 'note-mermaid-hint';
    hint.textContent = 'Ctrl+Enter applies · Esc cancels';
    pop.append(area, preview, hint);
    dom.append(pop);
    drawMermaid(preview, area.value);
    let timer = null;
    area.addEventListener('input', () => {
      clearTimeout(timer);
      timer = setTimeout(() => drawMermaid(preview, area.value), 300);
    });
    area.addEventListener('keydown', (ev) => {
      ev.stopPropagation();
      if (ev.key === 'Escape') {
        ev.preventDefault();
        closePop();
        return;
      }
      if (ev.key === 'Enter' && (ev.ctrlKey || ev.metaKey)) {
        ev.preventDefault();
        if (isReadonly?.()) return closePop();
        const pos = typeof getPos === 'function' ? getPos() : null;
        if (pos == null) return closePop();
        const from = pos + 1;
        const to = pos + node.nodeSize - 1;
        const text = area.value;
        const tr = view.state.tr;
        tr.replaceWith(from, to, text ? view.state.schema.text(text) : null);
        // CLOSED FIRST, and that order is the whole of it: `update()` runs
        // inside `dispatch` and skips the redraw while the popover is open
        // (`if (!pop)`), so applying with the popover still up left the
        // drawing showing the version before the edit — measured 2026-09-22.
        closePop();
        view.dispatch(tr);
      }
    });
    area.focus();
    area.select();
  };
  dom.addEventListener('click', (ev) => {
    ev.preventDefault();
    if (!pop) openPop();
  });
  // A card locked while its popover is open closes it: the lock is a state,
  // not a moment.
  const closeIfLocked = () => {
    if (pop && isReadonly?.()) closePop();
  };

  return {
    dom,
    // No `contentDOM`: the fence's text is drawn, not edited in place — the
    // popover is where it is edited.
    update(next) {
      if (next.type !== node.type || next.attrs.language !== 'mermaid') return false;
      node = next;
      closeIfLocked();
      if (!pop) drawMermaid(figure, next.textContent);
      return true;
    },
    // The diagram is OUR DOM: ProseMirror must not read its mutations as
    // document edits, and a click inside the popover is not a selection.
    ignoreMutation: () => true,
    stopEvent: () => true,
    destroy: closePop,
  };
}

// The IIFE's global. Named for what it is — this is not the whole of Crepe.
window.CrepeLean = { create };
