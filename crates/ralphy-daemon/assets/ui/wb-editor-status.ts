/* ---------------------------------------------------------------------------
   ralphy workbench — the status bar under a code editor

   One quiet line under each Monaco editor: the cursor or the selection, the
   line count with an estimate at a wrap column, a token estimate, the size,
   the line ending, and the wrap control. The wrap choice is one setting for
   every editor in this browser, kept by `WBView`.
--------------------------------------------------------------------------- */
import { setMarkup } from "./wb-html.ts";
import { need, needInput, required } from "./wb-dom-need.ts";
import { formatSize } from "./wb-properties-format.ts";
import { WBView, WRAP_COLUMN_MAX, WRAP_COLUMN_MIN, isWrapMode } from "./wb-client-view.ts";
import type { WrapMode } from "./wb-client-view.ts";
import type { EditorOptions, MonacoDisposable, MonacoEditor } from "./wb-monaco.ts";

export type WrapSetting = { mode: WrapMode; column: number };

const DEFAULT_COLUMN = 100;
// The common rule of thumb for English text and code; no vendor publishes
// an offline tokenizer, and the ports measure about ±10% (researched 2026-11-28).
const CHARS_PER_TOKEN = 4;
const RECOUNT_DELAY_MS = 150;
// The token cap of one Claude Code file read (about 25k, from memory, not measured).
const TOKENS_PER_READ = 25_000;

/** Lines a text would take if every line were cut at `column` characters. */
export function estimateWrappedLines(lines: readonly string[], column: number, tabSize: number) {
  let total = 0;
  for (const line of lines) total += Math.max(1, Math.ceil(visualWidth(line, tabSize) / column));
  return total;
}

function visualWidth(line: string, tabSize: number) {
  let width = 0;
  for (const ch of line) width += ch === "\t" ? tabSize - (width % tabSize) : 1;
  return width;
}

/** A rough token count for `chars` characters of text. */
export function estimateTokens(chars: number) {
  return Math.ceil(chars / CHARS_PER_TOKEN);
}

/** `950`, `2.4k`, `150k`, `1.3M`: a count short enough for the bar. */
export function shortCount(n: number) {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${scaled(n / 1000)}k`;
  return `${scaled(n / 1_000_000)}M`;
}

// One decimal under 10, none from there: `2.4`, `150`.
function scaled(v: number) {
  return v < 10 ? v.toFixed(1) : String(Math.round(v));
}

/** Reads an agent needs for `tokens`, at about 25k tokens a read. */
export function readsFor(tokens: number) {
  return Math.max(1, Math.ceil(tokens / TOKENS_PER_READ));
}

/** A bar text cut into a value, a unit, a value, a unit…: values get the accent. */
export type Pieces = string[];

/** `61 ln ≈74@100`: the lines, then the lines if cut at `column`. */
export function linesText(count: number, wrapped: number, column: number): Pieces {
  return [String(count), " ln ≈", String(wrapped), "@", String(column)];
}

/** `≈150k tok (6 rd)`: the token estimate and the reads it takes. */
export function tokensText(tokens: number): Pieces {
  return ["≈" + shortCount(tokens), " tok (", String(readsFor(tokens)), " rd)"];
}

/** `sel 412 ch ≈103 tok`: what a selection holds. */
export function selectionText(chars: number): Pieces {
  const tokens = "≈" + shortCount(estimateTokens(chars));
  return ["", "sel ", shortCount(chars), " ch ", tokens, " tok"];
}

/** `10.0 KB`: the number is the value, the unit is not. */
function sizeText(bytes: number): Pieces {
  const [value = "", unit = ""] = formatSize(bytes).split(" ");
  return [value, " " + unit];
}

function paint(doc: Document, el: HTMLElement, pieces: Pieces) {
  el.replaceChildren(
    ...pieces.map((text, i) => {
      const span = doc.createElement("span");
      span.className = i % 2 === 0 ? "editor-status-val" : "editor-status-unit";
      span.textContent = text;
      return span;
    }),
  );
}

/** The Monaco options of a wrap setting. `bounded` keeps the text inside a narrow pane. */
export function wrapOptions(setting: WrapSetting): EditorOptions {
  switch (setting.mode) {
    case "off":
      return { wordWrap: "off" };
    case "window":
      return { wordWrap: "on" };
    case "column":
      return { wordWrap: "bounded", wordWrapColumn: setting.column };
    default: {
      const never: never = setting.mode;
      return never;
    }
  }
}

/** A column a person typed, held to the range the field allows. */
export function clampColumn(raw: number) {
  if (!Number.isFinite(raw)) return DEFAULT_COLUMN;
  return Math.min(WRAP_COLUMN_MAX, Math.max(WRAP_COLUMN_MIN, Math.round(raw)));
}

/** The column in the field's text; text that is not a number keeps `current`. */
export function typedColumn(text: string, current: number) {
  const digits = text.trim();
  return /^\d+$/u.test(digits) ? clampColumn(Number(digits)) : current;
}

// Every open bar, so a change in one applies to all of them.
const liveBars = new Set<(setting: WrapSetting) => void>();

function savedWrap(fallback: WrapMode): WrapSetting {
  return WBView.read()?.wrap ?? { mode: fallback, column: DEFAULT_COLUMN };
}

function setWrap(setting: WrapSetting) {
  WBView.patch({ wrap: setting });
  for (const apply of liveBars) apply(setting);
}

/** The parts of a bar that change. */
type Parts = { [K in "cursor" | "lines" | "tokens" | "size" | "eol"]: HTMLElement } & {
  doc: Document;
  mode: HTMLSelectElement;
  column: HTMLInputElement;
};

function partsOf(doc: Document, el: HTMLElement): Parts {
  const part = (name: string) => need(el, `[data-part="${name}"]`);
  return {
    doc,
    cursor: part("cursor"),
    lines: part("lines"),
    tokens: part("tokens"),
    size: part("size"),
    eol: part("eol"),
    mode: required(el.querySelector<HTMLSelectElement>('[data-part="mode"]'), "the wrap menu"),
    column: needInput(el, '[data-part="column"]'),
  };
}

function paintCursor(parts: Parts, ed: MonacoEditor) {
  const sel = ed.getSelection();
  const model = ed.getModel();
  if (sel && model && !sel.isEmpty()) {
    paint(parts.doc, parts.cursor, selectionText(model.getValueInRange(sel).length));
    return;
  }
  const pos = ed.getPosition();
  paint(parts.doc, parts.cursor, pos ? [String(pos.lineNumber), ":", String(pos.column)] : []);
}

function paintCounts(parts: Parts, ed: MonacoEditor, column: number) {
  const model = ed.getModel();
  if (!model) return;
  const count = model.getLineCount();
  const wrapped = estimateWrappedLines(model.getLinesContent(), column, model.getOptions().tabSize);
  paint(parts.doc, parts.lines, linesText(count, wrapped, column));
  paint(parts.doc, parts.tokens, tokensText(estimateTokens(model.getValueLength())));
  paint(parts.doc, parts.size, sizeText(new TextEncoder().encode(model.getValue()).length));
  paint(parts.doc, parts.eol, ["", model.getEOL() === "\r\n" ? "CRLF" : "LF"]);
}

/**
 * Put the status bar after `host` and wire it to `ed`. `fallback` is the wrap
 * of this kind of file until the person picks one. Dispose it with the editor.
 */
export function mountEditorStatus(
  doc: Document,
  host: HTMLElement,
  ed: MonacoEditor,
  fallback: WrapMode,
): MonacoDisposable {
  const el = doc.createElement("div");
  el.className = "editor-status";
  setMarkup(
    el,
    `
  <span class="editor-status-item" data-part="cursor"
    title="Line and column, or the size of the selection"></span>
  <span class="editor-status-item" data-part="lines"
    title="Lines, then the estimated lines if each is cut at the wrap column"></span>
  <span class="editor-status-item" data-part="tokens"
    title="Estimated tokens (characters divided by 4), and the agent reads of about 25k tokens"
  ></span>
  <span class="editor-status-item" data-part="size"></span>
  <span class="editor-status-item" data-part="eol"></span>
  <label class="editor-status-item" title="Line wrap"><i class="bi bi-text-wrap"></i>
    <select class="editor-status-field" data-part="mode" aria-label="Line wrap">
      <option value="off">Off</option>
      <option value="window">Window</option>
      <option value="column">Column</option>
    </select></label>
  <input class="editor-status-field editor-status-num" data-part="column" type="text"
    inputmode="numeric" maxlength="3" spellcheck="false"
    title="Wrap column, also used for the line estimate" aria-label="Wrap column">`,
  );
  host.after(el);
  const parts = partsOf(doc, el);
  let setting = savedWrap(fallback);
  let timer: ReturnType<typeof setTimeout> | undefined;

  const apply = (next: WrapSetting) => {
    setting = next;
    parts.mode.value = next.mode;
    parts.column.value = String(next.column);
    ed.updateOptions(wrapOptions(next));
    paintCounts(parts, ed, next.column);
  };
  const recount = () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      paintCounts(parts, ed, setting.column);
    }, RECOUNT_DELAY_MS);
  };
  parts.mode.addEventListener("change", () => {
    const mode = parts.mode.value;
    if (isWrapMode(mode)) setWrap({ ...setting, mode });
  });
  parts.column.addEventListener("change", () => {
    setWrap({ ...setting, column: typedColumn(parts.column.value, setting.column) });
  });
  parts.column.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === "Escape") parts.column.blur();
  });
  const subs = [
    ed.onDidChangeModelContent(recount),
    ed.onDidChangeCursorSelection(() => {
      paintCursor(parts, ed);
    }),
  ];
  liveBars.add(apply);
  apply(setting);
  paintCursor(parts, ed);
  return {
    dispose() {
      clearTimeout(timer);
      liveBars.delete(apply);
      for (const sub of subs) sub.dispose();
      el.remove();
    },
  };
}
