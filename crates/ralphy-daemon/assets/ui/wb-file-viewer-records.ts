// The records of the file panes and the shapes they cross windows in: one
// record per open pane, and the guards of what a window posts to another.
import { isNullableString, isOptionalString, isRecord } from "./wb-api.ts";
import type { MonacoDiffEditor, MonacoDisposable, MonacoEditor } from "./wb-monaco.ts";

/**
 * One open file pane. `open` fills the first group; the rest appear as the
 * pane is used. `ed` is a diff editor on a `diff` pane and a code editor on
 * every other.
 */
export type PaneRecord = {
  id: string;
  project: string;
  label: string;
  path: string;
  kind: string;
  original: string | undefined;
  uid: number;
  editing: boolean;
  visible: boolean;
  detached: boolean;
  checkout: string | null;
  encoding: string;
  bom: boolean;
  el?: HTMLElement;
  ed?: MonacoEditor | MonacoDiffEditor | undefined;
  dirty?: boolean;
  mounting?: boolean;
  mountFailed?: boolean;
  fallbackEl?: HTMLElement;
  saveBtn?: HTMLElement | null;
  mirrorBtn?: HTMLElement | null;
  saveKey?: MonacoDisposable | undefined;
  ro?: ResizeObserver | undefined;
  encMenu?: HTMLElement;
  /** A find asked for while the editor was not mounted or the pane was off screen. */
  pendingFind?: string | null;
  /** The bytes of an external write a dirty pane has not taken yet. */
  pendingDisk?: string | undefined;
  mermaidPending?: HTMLElement[];
  hits?: HTMLElement[];
  hitIdx?: number;
};

/** A pane over the file's bytes. A record never changes between the two
 * kinds: a refused pane that gets bytes is replaced (`reopenRefused`). */
export type BytesRecord = PaneRecord & { content: string; refused: null };
/** A pane that holds the daemon's reason for serving no bytes. */
export type RefusedRecord = PaneRecord & { refused: string };
export type ViewerRecord = BytesRecord | RefusedRecord;

/** What `open` takes: the fields of a `FileDescriptor`, or the reason a pane has no bytes. */
export type OpenSpec = {
  id: string;
  project: string;
  label?: string;
  path: string;
  ftype: string;
  /** The diff's HEAD side. */
  original?: string;
  detached?: boolean;
  checkout?: string | null;
  encoding?: string | undefined;
  bom?: boolean | undefined;
} & ({ content: string; refused?: undefined } | { content?: undefined; refused: string });

/** Whether `v` has the shape `descOf` gives a file: what one window posts to
 * another when a file pane is detached or comes home. */
export function isFileDescriptor(v: unknown): v is FileDescriptor {
  return (
    isRecord(v) &&
    typeof v.project === "string" &&
    typeof v.label === "string" &&
    typeof v.path === "string" &&
    typeof v.ftype === "string" &&
    typeof v.content === "string" &&
    isNullableString(v.checkout) &&
    isOptionalString(v.encoding) &&
    typeof v.bom === "boolean"
  );
}

/** Whether `v` has the shape of the `workbench:open-request` detail this
 * module sends: the file a rendered link asks to open. */
export function isOpenRequest(v: unknown): v is OpenRequest {
  return (
    isRecord(v) &&
    isNullableString(v.project) &&
    typeof v.path === "string" &&
    isNullableString(v.checkout) &&
    isOptionalString(v.fragment) &&
    (v.as === undefined || v.as === "bytes")
  );
}
