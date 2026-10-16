/* ---------------------------------------------------------------------------
   The words of the Properties dialog, as pure functions: a size, a time, the
   type of a file, and the two rows that a folder's size fills. No `this`, no
   DOM, no socket, so each rule has one test.
   --------------------------------------------------------------------------- */

/** What `file.stat` says of the type of a file. */
export type StatType = NonNullable<ReplyOf<"file.stat">["type"]>;

/** The counts of one `tree.size` reply. */
export type SizeReply = ReplyOf<"tree.size">;

const UNITS = ["KB", "MB", "GB", "TB"];

/** A size for a person, then the exact bytes: `12.4 KB (12,698 bytes)`. Units
 * are 1024 bytes, as the file manager of the system shows them. */
export function formatBytes(bytes: number) {
  const exact = `${bytes.toLocaleString("en-US")} ${bytes === 1 ? "byte" : "bytes"}`;
  if (bytes < 1024) return exact;
  let value = bytes;
  let unit = -1;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(1)} ${UNITS[unit]} (${exact})`;
}

/** The time of a change, as `15 Oct 2026, 19:48`, in the time zone of the
 * browser unless a test names one. */
export function formatModified(ms: number, timeZone?: string) {
  return new Date(ms).toLocaleString("en-GB", {
    day: "numeric",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
    timeZone,
  });
}

/** The line for a time the system may not keep. */
export function modifiedText(ms: number | undefined) {
  return ms === undefined ? "Unknown" : formatModified(ms);
}

/** The line for a fact that is a yes or a no. */
export function yesNo(fact: boolean | undefined) {
  return fact ? "Yes" : "No";
}

/** The line for the type of a file. */
export function typeText(type: StatType | undefined) {
  return type ? typeLabel(type) : "Unknown";
}

/** What the file is, as the workbench would open it. */
export function typeLabel(type: StatType) {
  switch (type.class) {
    case "text":
      return `Text · ${type.encoding ?? "unknown encoding"}${type.bom ? " with BOM" : ""}`;
    case "image":
      return `Image · ${(type.image ?? "").toUpperCase()}`;
    case "binary":
      return "Binary";
    case "too_large":
      return "Too large to open in the workbench";
    default: {
      const unknown: never = type.class;
      return unknown;
    }
  }
}

const plural = (n: number, one: string, many: string) => `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;

/** The size of a folder: a lower bound, marked `≥`, when the walk stopped early. */
export function folderSize(reply: SizeReply) {
  return `${reply.truncated ? "≥ " : ""}${formatBytes(reply.bytes ?? 0)}`;
}

/** What a folder holds: `3 files, 1 folder`. */
export function folderContents(reply: SizeReply) {
  return `${plural(reply.files ?? 0, "file", "files")}, ${plural(reply.dirs ?? 0, "folder", "folders")}`;
}
