// The file viewers, by kind (ADR-0091 D1): which extensions open in each and
// how its pane is built. A viewer reads the file bytes route, never a verb,
// so a new viewer is its own module plus one entry here.
import { buildImage } from "./wb-file-viewer-read-only.ts";
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
