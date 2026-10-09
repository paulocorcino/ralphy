/* ---------------------------------------------------------------------------
   The workbench's rules about a file path, as pure functions: which viewer a
   file name gets, the directory that holds a rel path, a file tab's identity,
   the title of a create gesture, and the directories the daemon's Write path
   refuses.

   No `this`, no DOM, no socket. `app.ts` (the canvas tabs and the write
   seam), `wb-files.ts` (the FILES tree) and `wb-move-dialog.ts` (the move
   picker) each import the rules they apply, so each rule is written once.
   --------------------------------------------------------------------------- */
// Images the daemon serves as bytes (ADR-0049). The daemon holds the
// authoritative allowlist; this set only decides which VERB a click sends.
const IMAGE_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg"]);

// Neither renderable source nor image: refused.
const BINARY_EXT = new Set([
  "pdf", "zip", "gz",
  "tar", "rar", "7z", "exe", "dll", "so", "dylib", "bin", "class", "jar", "wasm",
  "mp3", "wav", "flac", "ogg", "mp4", "mov", "avi", "mkv", "webm", "woff",
  "woff2", "ttf", "eot", "otf",
]);

function extOf(name: any) {
  const n = name.toLowerCase();
  return n.includes(".") ? n.split(".").pop() : "";
}

// The directories the daemon's Write path refuses (`fswrite::PROTECTED_DIRS`),
// mirrored so the UI never offers a gesture that can only be refused.
// Case-insensitive: NTFS resolves `.GIT` to `.git`.
const PROTECTED_DIRS = [".git", ".ralphy"];

export function isProtectedDir(name: any) {
  return PROTECTED_DIRS.some((p) => name.toLowerCase() === p);
}

// Whether `rel` names or traverses a protected directory (the daemon's own
// component test), with the daemon's one carve-out: a note in the notes
// landing directory IS writable (ADR-0064 §5), so the tree may offer rename
// and delete on it. Exactly `.ralphy/notes/<name>.note`, spelled that way —
// the same narrow shape `fswrite::is_note_in_notes_dir` opens.
function isNoteInNotesDir(rel: any) {
  // `.` and empty segments are dropped first: `Path::components()` on the
  // daemon's side collapses them, so `.ralphy/./notes/x.note` is one path
  // there and would be two different answers here.
  const parts = rel.split("/").filter((p: any) => p && p !== ".");
  return (
    parts.length === 3 &&
    parts[0] === ".ralphy" &&
    parts[1] === "notes" &&
    parts[2].length > ".note".length &&
    parts[2].endsWith(".note")
  );
}

export function underProtectedDir(rel: any) {
  if (isNoteInNotesDir(rel)) return false;
  return rel.split("/").some(isProtectedDir);
}

// The title of a create gesture, one sentence with its word order kept
// whole (ADR-0065 §9). `dir` is "" for the top of the project.
export function newEntryTitle(kind: any, dir: any) {
  return `New ${kind} in ${dir || "the project root"}`;
}

// The directory containing `rel`; "" for a top-level entry (the repo root).
export function parentRel(rel: any) {
  const i = rel.lastIndexOf("/");
  return i < 0 ? "" : rel.slice(0, i);
}

// A file tab's identity (#406): project, path and — ONLY under a selected
// worktree — the checkout, so the same rel in two trees is two tabs (the
// primary's id is the pre-#406 spelling, byte for byte).
export function fileTabId(project: any, path: any, checkout: any) {
  return checkout ? `file:${project}@${checkout}:${path}` : `file:${project}:${path}`;
}

// Which viewer a file gets: markdown → rendered pane, image → image pane,
// a note → its CARD on the consoles stage (ADR-0064 §11, never a tab: two
// editors over one file is the thing that decision exists to prevent), other
// binaries refused, everything else source code.
export function classify(name: any) {
  const ext = extOf(name);
  if (ext === "note") return "note";
  if (ext === "md" || ext === "markdown") return "markdown";
  if (IMAGE_EXT.has(ext)) return "image";
  if (BINARY_EXT.has(ext)) return "binary";
  return "code";
}
