# Spike: hybrid-WYSIWYG markdown editor for notes (2026-09-22)

Companion note to [ADR-0064](adr/0064-notes-on-the-stage.md) §6. One HTML page mounting the
repo's own Monaco (AMD loader from `assets/ui/vendor/`), each candidate at pane size and as ten
240×180 cards, driven headless by Playwright 1.62 (Chromium); bundles built with esbuild 0.28.2.
The fixture exercised `#`/`##` headings, a task list, a table, a relative and an absolute link,
a mermaid fence, a rust fence, a quote and an ordered list. The page and its artefacts were
scratch and are not kept; this file is the record.

## Candidates

| | Milkdown Crepe 7.22.1 (lean, via `CrepeBuilder`) | ink-mde 0.34.0 |
|---|---|---|
| family | document model (ProseMirror), markdown serialised in/out | source-based live preview (CodeMirror 6) |
| licence | MIT | MIT |
| last release | 2026-08-12 (monthly cadence) | 2024-09-28 |
| bundle (esbuild, minified) | 716 KB JS + 21 KB CSS, one file each | 646 KB eager over 12 ESM chunks (2.0 MB as IIFE: language packs eager) |
| gzip | 237 KB + 4 KB | 706 KB (IIFE) |
| framework inside | `@vue/runtime-core` 37 KB (its own components) | none |
| coexists with Monaco AMD loader | yes â€” 0 errors, Monaco models intact | yes |
| first mount | 74â€“135 ms | 20 ms |
| 10 cards 240Ã—180 | 95 ms (~10 ms each) | 22 ms |
| heap, all three + 20 cards | 33.5 MB | (same page) |
| roundtrip of fixture | bullets `*`â†’ fixed with `remarkStringifyOptionsCtx {bullet:'-'}`; table cells re-padded, one trailing newline added â€” semantic-preserving | byte-identical |
| `## ` / `- [ ] ` input rules | yes, markup dissolves; `/` slash menu; block handle `+ â ¿`; selection toolbar; table editing | yes, markup stays visible (dimmed); tables never render as a grid |
| mermaid fence | plain code block (custom node view possible) | plain code block |

Features dropped from Crepe for the lean build: CodeMirror (code-block editing, âˆ’1.2 MB of language
packs), Latex (âˆ’263 KB JS, âˆ’1.4 MB katex CSS), ImageBlock, TopBar, AI.

## Reading

Crepe is the Notion feel: at card size the markup is gone, the title is set in the reading face,
checkboxes and tables are real controls. ink-mde is the Obsidian feel and its family cannot render a table.
Theme is CSS variables (`--crepe-*`), so ADR-0035 tokens map without patching the bundle; the
default theme's 64px inner padding must be overridden for a card.

Decision input for ADR-0064: Crepe lean, vendored as one JS + one CSS built once by esbuild with
the build script committed next to the artefact. ink-mde rejected (stale, no tables, markup visible).
Editor.js rejected earlier (JSON blocks, not markdown).
