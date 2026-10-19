/* ---------------------------------------------------------------------------
   The upload dialog (ADR-0036 amendment 2026-10-19): an Alpine component
   around its modal.

   The files send `workbench:upload-open` with the project, the target folder
   and the files the operator dropped or picked. The dialog lists them, refuses
   each file over the upload limit of the project (`files.upload_max_mb`), and
   asks once what to do with the names that already exist in the folder:
   replace, keep both, or skip. "Upload" gives the files to the `uploads`
   store, which sends them.

   It reaches `shell()` only through the names in its `uses` (ADR-0073 D4).
   `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";
import { sendWindow } from "./wb-events.ts";
import { underProtectedDir } from "./wb-file-paths.ts";
import type { OnConflict } from "./wb-uploads-store.ts";

/** `files.upload_max_mb` of the daemon when the settings name none. */
const DEFAULT_MAX_MB = 25;
/** A larger setting is read as this, as the daemon reads it. */
const CEILING_MB = 1024;

/** A file as the dialog shows it. */
type UploadRow = { name: string; size: number; tooBig: boolean; exists: boolean };

/** The choice for the names that exist. */
type ExistingChoice = "replace" | "keep_both" | "skip";

/** What `workbench:upload-open` carries. */
export type UploadOpen = { repo: string; folder: string; files: (Blob & { name: string })[] };

/** The dialog's state. */
type UploadAsk = {
  open: boolean;
  repo: string;
  folder: string;
  rows: UploadRow[];
  limitMb: number;
  choice: ExistingChoice;
  busy: boolean;
  error: string;
};

// The files of the open dialog. Outside the component: Alpine's Proxy would
// wrap each `File`, and a `File` read through a Proxy throws.
let pending: (Blob & { name: string })[] = [];

/** The limit in MiB from a `config.get` value, as the daemon reads it. */
export function limitMb(value: unknown): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 1) return DEFAULT_MAX_MB;
  return Math.min(value, CEILING_MB);
}

/** The size of a file in words. */
export function sizeText(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** What the last part of a file does, by the dialog's choice; `null` skips it. */
export function conflictFor(row: UploadRow, choice: ExistingChoice): OnConflict | null {
  if (!row.exists) return "fail";
  if (choice === "skip") return null;
  return choice;
}

export function wbUploadDialog() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component([], {
    upload: {
      open: false,
      repo: "",
      folder: "",
      rows: [],
      limitMb: DEFAULT_MAX_MB,
      choice: "keep_both",
      busy: false,
      error: "",
    } as UploadAsk,

    // On `workbench:upload-open`.
    async openUpload(ask: UploadOpen) {
      pending = [...ask.files];
      this.upload = {
        open: true,
        repo: ask.repo,
        folder: ask.folder,
        rows: pending.map((f) => ({ name: f.name, size: f.size, tooBig: false, exists: false })),
        limitMb: DEFAULT_MAX_MB,
        choice: "keep_both",
        busy: true,
        error: "",
      };
      const [config, listing] = await Promise.all([
        WBDaemon.observe("config.get", { repo: ask.repo }).catch(() => null),
        WBDaemon.observe("tree.list", { repo: ask.repo, path: ask.folder }).catch(() => null),
      ]);
      if (!this.upload.open || this.upload.repo !== ask.repo || this.upload.folder !== ask.folder) return;
      this.upload.busy = false;
      if (!listing || WBFail.isError(listing) || !Array.isArray(listing.entries)) {
        this.upload.error = WBFail.failed(listing, "Could not list the folder: the daemon gave no reason.");
        return;
      }
      const limit = limitMb(config && !WBFail.isError(config) ? config.config?.["files.upload_max_mb"] : undefined);
      // NTFS and APFS match names without case, so a name that differs only
      // in case is asked about too.
      const names = new Set(listing.entries.map((e) => e.name.toLowerCase()));
      this.upload.limitMb = limit;
      this.upload.rows = this.upload.rows.map((r) => ({
        ...r,
        tooBig: r.size > limit * 1024 * 1024,
        exists: names.has(r.name.toLowerCase()),
      }));
    },

    // What a row says beside its name.
    uploadRowText(row: UploadRow) {
      if (row.tooBig) return "Over the upload limit";
      return row.exists ? "Exists" : sizeText(row.size);
    },

    // Whether any file to send has a name that exists in the folder.
    uploadHasExisting() {
      return this.upload.rows.some((r) => r.exists && !r.tooBig);
    },

    // The files "Upload" sends, with what their last part does.
    uploadRequests() {
      return pending.flatMap((file, i) => {
        const row = this.upload.rows[i];
        const onConflict = row && !row.tooBig ? conflictFor(row, this.upload.choice) : null;
        return onConflict ? [{ file, repo: this.upload.repo, folder: this.upload.folder, onConflict }] : [];
      });
    },

    uploadBlocked() {
      return this.upload.busy || !!this.upload.error || this.uploadRequests().length === 0;
    },

    uploadCancel() {
      this.upload.open = false;
      pending = [];
    },

    uploadConfirm() {
      if (this.uploadBlocked()) return;
      const requests = this.uploadRequests();
      this.upload.open = false;
      pending = [];
      this.$store.uploads.add(requests);
    },
  });
}

/** What the drop needs from the files tree. */
type DropTree = {
  createDir(node: WunderbaumNode | null): string;
  $store: import("./wb-alpine.ts").AlpineMagics["$store"];
};

/** Whether a drag carries files from the operator's computer. */
function carriesFiles(event: DragEvent): boolean {
  return !!event.dataTransfer?.types.includes("Files");
}

/** Open the dialog for the files dropped or picked on `node` (`null` is the
 * project root). A drop on a file means its folder. A folder in the drop, a
 * worktree and a folder inside `.git` or `.ralphy` are refused with a message. */
export function askUpload(tree: DropTree, node: WunderbaumNode | null, files: ArrayLike<Blob & { name: string }>, items?: DataTransferItemList) {
  const flash = (msg: string) => tree.$store.modals.flash(msg);
  const dropped = items ? Array.from(items) : [];
  if (dropped.some((item) => item.webkitGetAsEntry?.()?.isDirectory)) {
    flash("Upload takes files, not folders. Drop the files of the folder instead.");
    return;
  }
  if (files.length === 0) return;
  const repo = tree.$store.projects.openSlug;
  if (!repo) return;
  if (tree.$store.checkouts.checkoutOf(repo)) {
    flash("Uploading into a worktree is not available yet.");
    return;
  }
  const folder = tree.createDir(node);
  if (underProtectedDir(folder)) {
    flash("Ralphy cannot write a file in this folder.");
    return;
  }
  sendWindow(window, "workbench:upload-open", { repo, folder, files: Array.from(files) });
}

/** The `dnd` options of the files tree: a drop of files from the computer,
 * and no drag of the tree's own rows (no `dragStart`). */
export function treeDrop(tree: DropTree): WunderbaumOptions["dnd"] {
  return {
    preventLazyParents: false,
    dragEnter: (e) => (carriesFiles(e.event) ? "over" : false),
    drop: (e) => {
      const data = e.event.dataTransfer;
      if (data) askUpload(tree, e.node, data.files, data.items);
    },
  };
}

/** A drop on the empty space of the tree is a drop on the project root. Its
 * default would open the file in the tab, so it is stopped here. A drop on a
 * row is Wunderbaum's (`treeDrop`). */
export function guardEmptySpace(host: HTMLElement, tree: DropTree) {
  host.addEventListener("dragover", (ev: DragEvent) => {
    if (carriesFiles(ev)) ev.preventDefault();
  });
  host.addEventListener("drop", (ev: DragEvent) => {
    if (!carriesFiles(ev)) return;
    ev.preventDefault();
    if (mar10.Wunderbaum.getNode(ev)) return;
    const data = ev.dataTransfer;
    if (data) askUpload(tree, null, data.files, data.items);
  });
}
