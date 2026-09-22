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
import { editorViewCtx, remarkStringifyOptionsCtx } from '@milkdown/kit/core';
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

// The IIFE's global. Named for what it is — this is not the whole of Crepe.
window.CrepeLean = { create };
