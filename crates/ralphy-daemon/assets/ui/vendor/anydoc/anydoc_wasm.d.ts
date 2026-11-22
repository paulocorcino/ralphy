/* tslint:disable */
/* eslint-disable */
/**
 * Input format, named after the extension that identifies it. Container
 * variants that share a parser (`.docm`, `.xlsm`, `.ppsx`, ...) map onto
 * these.
 */

export type Format = "doc" | "docx" | "odt" | "pdf" | "ppt" | "pptx" | "rtf" | "epub" | "xlsx" | "ods" | "odp" | "csv";

/**
 * `code` on the `Error` a failed conversion throws. Conversion fails only
 * when no complete Markdown could be produced; producer quirks are
 * recovered or skipped instead. The crate's `io` code has no wasm
 * counterpart: there is no filesystem to read from.
 */
export type ConvertErrorCode =
/** Unknown format, or one that cannot be converted. */
| 'unsupported'
/**
 * Pages of a PDF are scanned or image-only and need OCR, which anydoc does
 * not do. The error is a `NeedsOcrError` naming them.
 */
| 'needsOcr'
/** Structurally unusable: no meaningful content could be extracted. */
| 'malformed'
/** Encrypted or password-protected. */
| 'encrypted'
/** Crossed a fixed safety limit (decompression, nesting, node count). */
| 'resourceLimit'
/** A part required for any meaningful output is absent. */
| 'missingPart'

/** The error thrown for a PDF with pages that need OCR. */
export interface NeedsOcrError extends Error {
    code: 'needsOcr'
    /** 1-indexed pages that need OCR. */
    pages: number[]
    /** Pages in the document. */
    pageCount: number
}

export interface Document {
    blocks: Array<Block>
    /**
     * Footnote and endnote bodies, referenced from text by a `noteRef`
     * inline.
     */
    notes: Array<Note>
    assets: Array<Asset>
}

export type BlockKind =
| 'heading'
| 'paragraph'
| 'list'
| 'table'
| 'blockQuote'
| 'codeBlock'
| 'rule'
| 'math'

export interface Block {
    kind: BlockKind
    /** heading: 1-6. */
    level?: number
    /** heading: stable anchor id when the document targets this heading. */
    anchor?: string
    /** heading, paragraph. */
    content?: Array<Inline>
    list?: List
    table?: Table
    /** blockQuote. */
    blocks?: Array<Block>
    /** codeBlock. */
    lang?: string
    /** codeBlock, math (LaTeX source without delimiters). */
    text?: string
}

export type InlineKind =
| 'text'
| 'link'
| 'image'
/** Zero-width marker for an internal link target at this position. */
| 'anchor'
| 'noteRef'
| 'lineBreak'
/** An inline formula. */
| 'math'
/** A checkbox control. */
| 'checkbox'

export interface Inline {
    kind: InlineKind
    /** text; math (LaTeX source without delimiters). */
    text?: string
    /** text. */
    style?: Style
    /** link. */
    content?: Array<Inline>
    /** link. */
    target?: LinkTarget
    /** image. */
    alt?: string
    /** image. */
    source?: ImageSource
    /** anchor: the anchor id. */
    anchor?: string
    /** noteRef: the id of the note in `Document.notes`. */
    noteId?: string
    /** checkbox: its state. */
    checked?: boolean
}

/** Fully resolved character style. */
export interface Style {
    bold: boolean
    italic: boolean
    strike: boolean
    code: boolean
}

export type LinkTargetKind =
/** Absolute URL with a scheme. */
| 'external'
/** Scheme-less relative reference, preserved as written. */
| 'relative'
/** Internal target: a heading anchor or an `anchor` inline. */
| 'anchor'

export interface LinkTarget {
    kind: LinkTargetKind
    /** The URL, relative reference, or anchor id. */
    value: string
}

export type ImageSourceKind =
/** Absolute URL with a scheme. */
| 'external'
/** Embedded image, carried in `Document.assets`. */
| 'asset'
/**
 * No usable source: the image's part is missing or unreadable and it has
 * no URL. Only the alt text remains.
 */
| 'unavailable'

export interface ImageSource {
    kind: ImageSourceKind
    /** external. */
    url?: string
    /** asset: index into `Document.assets`. */
    assetId?: number
}

/** The marker family a list uses in the source document. */
export type MarkerKind =
| 'bullet'
| 'decimal'
| 'lowerAlpha'
| 'upperAlpha'
| 'lowerRoman'
| 'upperRoman'

export interface List {
    marker: MarkerKind
    /** Ordinal the first item counts from. */
    start: number
    items: Array<ListItem>
}

export interface ListItem {
    blocks: Array<Block>
    /**
     * Literal marker text that overrides the list marker when the source
     * number text cannot be reproduced from the marker and position alone
     * (composite number text such as `1-a)`).
     */
    markerLabel?: string
}

export type TableKind =
/** A real data table. */
| 'data'
/** Layout scaffolding (text boxes, positioning tables). */
| 'layout'

/**
 * Canonical table grid: every logical grid position appears exactly once.
 * Content and spans live on the origin slot, and each position a span covers
 * holds a `covered` slot pointing back at that origin.
 */
export interface Table {
    grid: Array<Array<CellSlot>>
    /** Number of leading rows that are header rows (0 = no header). */
    headerRows: number
    kind: TableKind
}

export type CellSlotKind = 'origin' | 'covered'

export interface CellSlot {
    kind: CellSlotKind
    /** origin. */
    cell?: Cell
    /** covered: row of the origin this position belongs to. */
    originRow?: number
    /** covered: column of the origin this position belongs to. */
    originCol?: number
}

export interface Cell {
    blocks: Array<Block>
    colSpan: number
    rowSpan: number
}

export type NoteKind = 'footnote' | 'endnote'

export interface Note {
    id: string
    kind: NoteKind
    blocks: Array<Block>
}

/**
 * An embedded binary asset (image, object payload). Bytes are always
 * retained, so a document stays self-contained.
 */
export interface Asset {
    /** Index into `Document.assets`, as referenced by an image source. */
    id: number
    /** MIME type, e.g. `image/png`. */
    mediaType: string
    /** Package part or stream the asset came from, for provenance. */
    originPart: string
    data: Uint8Array
}



/**
 * Detect the format from the content itself: the signature and identity each
 * container specification designates (PDF header, RTF open group, OLE stream
 * names, ZIP package mimetype/content types). Plain-text formats (CSV) carry
 * no signature and return `undefined`; so does anything unrecognized.
 */
export function formatFromBytes(bytes: Uint8Array): Format | undefined;

/**
 * The format an extension names, with or without a leading dot.
 */
export function formatFromExtension(extension: string): Format | undefined;

/**
 * The format a path's (or file name's) extension names.
 */
export function formatFromPath(path: string): Format | undefined;

/**
 * Parse an in-memory document into the document model, which also carries
 * the embedded assets. Without a format, it is detected from the content.
 *
 * Unsupported for `pdf`: PDF conversion produces Markdown directly and has
 * no document-model form; use `toMarkdownBytes`.
 *
 * Throws an `Error` carrying a `ConvertErrorCode` on `code`.
 */
export function toDocument(bytes: Uint8Array, format?: Format | null): Document;

/**
 * Convert an in-memory document to Markdown. Without a format, it is
 * detected from the content, which signature-less formats (CSV) have to name
 * explicitly.
 *
 * Throws an `Error` carrying a `ConvertErrorCode` on `code`.
 */
export function toMarkdownBytes(bytes: Uint8Array, format?: Format | null): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly formatFromBytes: (a: number, b: number) => number;
    readonly formatFromExtension: (a: number, b: number) => number;
    readonly formatFromPath: (a: number, b: number) => number;
    readonly toDocument: (a: number, b: number, c: number, d: number) => void;
    readonly toMarkdownBytes: (a: number, b: number, c: number, d: number) => void;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_export3: (a: number) => void;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
    readonly __wbindgen_export4: (a: number, b: number, c: number) => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
