/* ---------------------------------------------------------------------------
   The workbench's rules about a file path, as pure functions: which viewer a
   file name gets and its icon in the FILES tree, the directory that holds a
   rel path, a file tab's identity,
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

function extOf(name: string) {
  const n = name.toLowerCase();
  return n.includes(".") ? (n.split(".").pop() as string) : "";
}

// The directories the daemon's Write path refuses (`fswrite::PROTECTED_DIRS`),
// mirrored so the UI never offers a gesture that can only be refused.
// Case-insensitive: NTFS resolves `.GIT` to `.git`.
const PROTECTED_DIRS = [".git", ".ralphy"];

export function isProtectedDir(name: string) {
  return PROTECTED_DIRS.some((p) => name.toLowerCase() === p);
}

// Whether `rel` names or traverses a protected directory (the daemon's own
// component test), with the daemon's one carve-out: a note in the notes
// landing directory IS writable (ADR-0064 §5), so the tree may offer rename
// and delete on it. Exactly `.ralphy/notes/<name>.note`, spelled that way —
// the same narrow shape `fswrite::is_note_in_notes_dir` opens.
function isNoteInNotesDir(rel: string) {
  // `.` and empty segments are dropped first: `Path::components()` on the
  // daemon's side collapses them, so `.ralphy/./notes/x.note` is one path
  // there and would be two different answers here.
  const parts = rel.split("/").filter((p) => p && p !== ".");
  return (
    parts.length === 3 &&
    parts[0] === ".ralphy" &&
    parts[1] === "notes" &&
    parts[2].length > ".note".length &&
    parts[2].endsWith(".note")
  );
}

export function underProtectedDir(rel: string) {
  if (isNoteInNotesDir(rel)) return false;
  return rel.split("/").some(isProtectedDir);
}

// The title of a create gesture, one sentence with its word order kept
// whole (ADR-0065 §9). `dir` is "" for the top of the project.
export function newEntryTitle(kind: string, dir: string) {
  return `New ${kind} in ${dir || "the project root"}`;
}

// The absolute path of `rel` below the project's `root`, in the root's own
// separator, so it pastes into a native shell. `rel` empty is the root.
export function fullPath(root: string, rel: string) {
  if (!rel) return root;
  // Windows by SHAPE (drive letter or UNC lead), not by "contains a backslash".
  const sep = /^[A-Za-z]:/.test(root) || root.startsWith("\\\\") ? "\\" : "/";
  // Trim a trailing separator so a drive root (`C:\`) does not double it.
  const base = root.replace(/[\\/]+$/, "");
  return base + sep + rel.split("/").join(sep);
}

// The directory containing `rel`; "" for a top-level entry (the repo root).
export function parentRel(rel: string) {
  const i = rel.lastIndexOf("/");
  return i < 0 ? "" : rel.slice(0, i);
}

// A file tab's identity (#406): project, path and — ONLY under a selected
// worktree — the checkout, so the same rel in two trees is two tabs (the
// primary's id is the pre-#406 spelling, byte for byte).
export function fileTabId(project: string, path: string, checkout: string | null | undefined) {
  return checkout ? `file:${project}@${checkout}:${path}` : `file:${project}:${path}`;
}

// Which viewer a file gets: markdown, a Mermaid diagram (`.mmd`) and a
// database markup file (`.dbml`) → rendered pane, image → image pane,
// a note → its CARD on the consoles stage (ADR-0064 §11, never a tab: two
// editors over one file is the thing that decision exists to prevent), other
// binaries refused, everything else source code.
export function classify(name: string) {
  const ext = extOf(name);
  if (ext === "note") return "note";
  if (ext === "md" || ext === "markdown") return "markdown";
  if (ext === "mmd") return "mermaid";
  if (ext === "dbml") return "dbml";
  if (IMAGE_EXT.has(ext)) return "image";
  if (BINARY_EXT.has(ext)) return "binary";
  return "code";
}

// The icon of a file's canvas tab, by the kind `classify` gave it.
export function tabIcon(kind: string) {
  switch (kind) {
    case "markdown":
      return "bi bi-file-earmark-text";
    case "image":
      return "bi bi-file-earmark-image";
    case "mermaid":
      return "bi bi-diagram-3";
    case "dbml":
      return "bi bi-table";
    default:
      return "bi bi-file-earmark-code";
  }
}

// The icon of a file in the FILES tree, by its name: a Devicon glyph or a
// Bootstrap icon for the extensions the tree knows, a plain file otherwise.
const TREE_ICONS: Record<string, string> = {
  ts: "devicon-typescript-plain colored",
  tsx: "devicon-typescript-plain colored",
  js: "devicon-javascript-plain colored",
  mjs: "devicon-javascript-plain colored",
  cjs: "devicon-javascript-plain colored",
  json: "devicon-json-plain colored",
  md: "devicon-markdown-plain md-glyph",
  rs: "devicon-rust-plain rs-glyph",
  css: "devicon-css3-plain colored",
  html: "devicon-html5-plain colored",
  prisma: "devicon-prisma-plain colored",
  png: "bi bi-image",
  jpg: "bi bi-image",
  jpeg: "bi bi-image",
  gif: "bi bi-image",
  svg: "bi bi-image",
  toml: "bi bi-gear",
  yml: "bi bi-gear",
  yaml: "bi bi-gear",
  note: "bi bi-sticky",
  mmd: "bi bi-diagram-3",
  dbml: "bi bi-table",
};

export function treeIcon(title: string) {
  const name = title.toLowerCase();
  if (name.endsWith("lock") || name === "package-lock.json") return "devicon-json-plain colored";
  return TREE_ICONS[extOf(name)] || "bi bi-file-earmark";
}
