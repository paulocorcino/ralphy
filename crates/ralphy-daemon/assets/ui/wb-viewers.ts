// The file viewers, by kind (ADR-0091 D1): which extensions open in each and
// how its pane is built. A viewer reads the file bytes route, or the one
// verb its kind needs (`file.entries`), so a new viewer is its own module
// plus one entry here.
import { buildImage } from "./wb-file-viewer-read-only.ts";
import { buildArchive } from "./wb-viewer-archive.ts";
import { buildDocument } from "./wb-viewer-document.ts";
import { buildDocx } from "./wb-viewer-docx.ts";
import { buildFont } from "./wb-viewer-font.ts";
import { buildMedia } from "./wb-viewer-media.ts";
import { buildSheet } from "./wb-viewer-sheet.ts";
import { buildPdf } from "./wb-viewer-pdf.ts";
import type { ViewerReadOnlyDeps } from "./wb-file-viewer-read-only.ts";
import type { BytesRecord } from "./wb-file-viewer-records.ts";

/** One viewer: the extensions it opens (lower case) and its pane builder. */
export type ViewerEntry = {
  extensions: readonly string[];
  build: (host: ViewerReadOnlyDeps, rec: BytesRecord) => void;
};

export const VIEWERS: Record<string, ViewerEntry> = {
  image: {
    extensions: ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "avif"],
    build: buildImage,
  },
  archive: {
    extensions: ["zip", "tar", "tar.gz", "tgz"],
    build: buildArchive,
  },
  media: {
    extensions: ["mp4", "m4a", "webm", "mp3", "wav", "ogg", "oga", "ogv"],
    build: buildMedia,
  },
  font: {
    extensions: ["ttf", "otf", "woff", "woff2"],
    build: buildFont,
  },
  docx: {
    extensions: ["docx"],
    build: buildDocx,
  },
  document: {
    extensions: ["odt"],
    build: buildDocument,
  },
  sheet: {
    extensions: ["xlsx", "ods"],
    build: buildSheet,
  },
  pdf: {
    extensions: ["pdf"],
    build: buildPdf,
  },
};

/** The viewer kind of a lower-case extension, or null when no viewer opens it. */
export function viewerKindOf(ext: string): string | null {
  for (const [kind, entry] of Object.entries(VIEWERS)) {
    if (entry.extensions.includes(ext)) return kind;
  }
  return null;
}

/** The viewer of `kind`, or undefined when the kind is not a viewer's. */
export function viewerOf(kind: string): ViewerEntry | undefined {
  return Object.hasOwn(VIEWERS, kind) ? VIEWERS[kind] : undefined;
}
