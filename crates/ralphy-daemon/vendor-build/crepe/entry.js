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
//   block-edit    — the slash menu and the drag handle; the Notion feel itself
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
  editorViewOptionsCtx,
  remarkStringifyOptionsCtx,
} from '@milkdown/kit/core';
import { listenerCtx } from '@milkdown/kit/plugin/listener';

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
    .addFeature(blockEdit)
    .addFeature(placeholder, { text: hint ?? 'Write a note…' });

  builder.editor.config((ctx) => {
    // A ```mermaid fence DRAWS (ADR-0064 §15). A node view and not a
    // post-render pass: ProseMirror owns this DOM and reconciles anything
    // written into it away within a frame (measured, §10's ring). Every other
    // code block keeps the plain editable shape.
    ctx.update(editorViewOptionsCtx, (prev) => ({
      ...prev,
      nodeViews: { ...(prev.nodeViews || {}), code_block: codeBlockView },
    }));
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
    };
  });
}

// ---- the mermaid node view (ADR-0064 §15) ------------------------------------

// The page's own `mermaid` and `DOMPurify` — the same two the file viewer
// draws with (`wb-viewer.js`), so a diagram looks identical in a note and in a
// README, and the SVG meets the same sanitizer. Absent (a shell that loaded
// neither), a mermaid fence stays a code block and nothing throws.
let mermaidReady = false;
function drawMermaid(host, source) {
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
      theme: 'dark',
      htmlLabels: false,
      flowchart: { htmlLabels: false },
    });
    mermaidReady = true;
  }
  const id = 'note-mermaid-' + Math.random().toString(36).slice(2, 10);
  window.mermaid
    .render(id, source)
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
function codeBlockView(node, view, getPos) {
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
    if (pop) return;
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
        const pos = typeof getPos === 'function' ? getPos() : null;
        if (pos == null) return closePop();
        const from = pos + 1;
        const to = pos + node.nodeSize - 1;
        const text = area.value;
        const tr = view.state.tr;
        tr.replaceWith(from, to, text ? view.state.schema.text(text) : null);
        view.dispatch(tr);
        closePop();
      }
    });
    area.focus();
    area.select();
  };
  dom.addEventListener('click', (ev) => {
    ev.preventDefault();
    if (!pop) openPop();
  });

  return {
    dom,
    // No `contentDOM`: the fence's text is drawn, not edited in place — the
    // popover is where it is edited.
    update(next) {
      if (next.type !== node.type || next.attrs.language !== 'mermaid') return false;
      node = next;
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
