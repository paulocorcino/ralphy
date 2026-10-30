/* ---------------------------------------------------------------------------
   The files the operator uploads to the open project, and the queue that
   sends them: the Alpine store `uploads` (ADR-0036 amendment 2026-10-19).

   The store is the one owner of this fact. Its fields change only through
   its own methods. The drop dialog (wb-files-upload.ts) adds files with
   `add`; the progress card at the bottom of the files panel reads `items`
   and calls `cancel`, `retry` and `clearFinished`.

   It sends one file at a time and one part at a time. Each part is read from
   the `File` with `File.slice()`, so the page never holds a whole file. Each
   item keeps its own project, so a change of project does not stop it. The
   daemon confirms what it holds: a part at a wrong offset answers `expected`,
   and the next part starts there. A failed request stops the item; `retry`
   starts again from the confirmed offset.

   `main.ts` registers it as `uploads`. `uploadsStore` builds it without
   Alpine, so `node --test` can test it (ADR-0073 D3, D6).
   --------------------------------------------------------------------------- */
import type { Messages } from "./wb-messages.ts";

/** The largest part, in bytes: `uploads::PART_MAX` of the daemon. */
export const PART_BYTES = 1024 * 1024;

/** What the last part does when the target exists. */
export type OnConflict = "fail" | "replace" | "keep_both";

/** Where an item is in the queue. */
export type UploadState = "waiting" | "sending" | "failed" | "done" | "cancelled";

/** One file to upload. */
export type UploadItem = {
  key: number;
  id: string;
  name: string;
  size: number;
  repo: string;
  /** The folder of the target, relative to the root; "" is the root. */
  folder: string;
  onConflict: OnConflict;
  /** The bytes the daemon confirmed. */
  sent: number;
  state: UploadState;
  /** Why the item stopped, in words for the operator. */
  error: string;
  /** Where the file landed, once done. */
  path: string;
};

/** What `add` takes for one file. */
export type UploadRequest = {
  file: Blob & { name: string };
  repo: string;
  folder: string;
  onConflict: OnConflict;
};

/** The reply of one part. */
type PartReply = {
  status?: string;
  reason?: string;
  message?: string;
  expected?: number;
  received?: number;
  path?: string;
};

/** What `main.ts` gives the store. */
export type UploadsStoreDeps = {
  /** The door for operator messages. */
  messages: Messages;
  /** Sends one part; the default is the daemon door. */
  send?: (payload: CommandPayload) => Promise<PartReply>;
};

/** A random upload id: 32 hex digits, as the daemon asks. */
export function uploadId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** Base64 of `bytes`, in slices small enough for `String.fromCharCode`. */
export function toBase64(bytes: Uint8Array): string {
  let text = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(text);
}

/** The sentence for a part the daemon refused. */
export function refusalText(reply: PartReply | null): string {
  if (!reply) return "The upload stopped: the daemon did not answer. Try again.";
  if (reply.message === "unknown verb")
    return "This host runs an older daemon. Update Ralphy on it to upload files.";
  switch (reply.reason) {
    case "too large":
      return "The file is larger than the upload limit of this project.";
    case "conflict":
      return "A file with this name exists now.";
    case "refused":
      return "Ralphy cannot write a file there.";
    case undefined:
    default:
      return `The daemon refused the upload: ${reply.reason || reply.message || "no reason given"}.`;
  }
}

/** The target of an item, relative to the root. */
function targetOf(item: UploadItem): string {
  return item.folder ? `${item.folder}/${item.name}` : item.name;
}

/** The next part of `item`, read from `file` at the offset the daemon holds.
 * Only the part that ends the file names the conflict choice. */
async function partOf(item: UploadItem, file: Blob): Promise<CommandPayload> {
  const end = Math.min(item.sent + PART_BYTES, item.size);
  const bytes = new Uint8Array(await file.slice(item.sent, end).arrayBuffer());
  const payload: CommandPayload = {
    repo: item.repo,
    path: targetOf(item),
    id: item.id,
    offset: item.sent,
    total: item.size,
    data: toBase64(bytes),
  };
  if (end === item.size) payload.on_conflict = item.onConflict;
  return payload;
}

/** The offset to send again from after an `offset` refusal, or `null` when
 * the reply is not one, or names the same offset as the last one. */
function resendFrom(reply: PartReply | null, seen: { expected: number }): number | null {
  if (
    reply?.reason !== "offset" ||
    typeof reply.expected !== "number" ||
    reply.expected === seen.expected
  )
    return null;
  seen.expected = reply.expected;
  return reply.expected;
}

export function uploadsStore(deps: UploadsStoreDeps) {
  const { messages } = deps;
  const send =
    deps.send || ((payload: CommandPayload) => window.WBDaemon.write("file.upload", payload));
  // The `File` of each item, by key: kept out of `items`, which Alpine wraps.
  const files = new Map<number, Blob & { name: string }>();
  let nextKey = 1;
  let running = false;
  return {
    items: [] as UploadItem[],

    // Add files to the end of the queue and start it.
    add(requests: UploadRequest[]) {
      for (const r of requests) {
        const key = nextKey++;
        files.set(key, r.file);
        this.items.push({
          key,
          id: uploadId(),
          name: r.file.name,
          size: r.file.size,
          repo: r.repo,
          folder: r.folder,
          onConflict: r.onConflict,
          sent: 0,
          state: "waiting",
          error: "",
          path: "",
        });
      }
      return this.pump();
    },

    // Stop an item. The daemon deletes its part file within one hour.
    cancel(key: number) {
      const item = this.items.find((i) => i.key === key);
      if (!item || item.state === "done") return;
      item.state = "cancelled";
      files.delete(key);
    },

    // Send a failed item again, from the offset the daemon confirmed.
    retry(key: number) {
      const item = this.items.find((i) => i.key === key);
      if (!item || item.state !== "failed") return undefined;
      item.state = "waiting";
      item.error = "";
      return this.pump();
    },

    // Drop the items that are done or cancelled from the card.
    clearFinished() {
      this.items = this.items.filter((i) => i.state !== "done" && i.state !== "cancelled");
    },

    // How much of an item the daemon holds, in whole percent.
    percent(item: UploadItem) {
      return item.size === 0 ? 100 : Math.floor((item.sent * 100) / item.size);
    },

    // The state of an item in words, for the card.
    stateText(item: UploadItem) {
      switch (item.state) {
        case "waiting":
          return "Waiting";
        case "sending":
          return `${this.percent(item)}%`;
        case "failed":
          return "Stopped";
        case "done":
          return "Done";
        case "cancelled":
          return "Cancelled";
        default: {
          const never: never = item.state;
          return never;
        }
      }
    },

    // Whether the card has anything to show.
    get visible() {
      return this.items.length > 0;
    },

    // Send the waiting items, one at a time. A second call while one runs
    // returns at once: the running loop takes the new items too.
    async pump() {
      if (running) return;
      running = true;
      try {
        let item: UploadItem | undefined;
        while ((item = this.items.find((i) => i.state === "waiting"))) {
          await this.sendItem(item);
        }
      } finally {
        running = false;
      }
    },

    async sendItem(item: UploadItem) {
      const file = files.get(item.key);
      if (!file) return;
      item.state = "sending";
      // An offset the daemon names twice in a row is not progress.
      const seen = { expected: -1 };
      while (item.state === "sending") {
        const reply = await send(await partOf(item, file)).catch(() => null);
        if ((item.state as UploadState) === "cancelled") return;
        this.applyReply(item, reply, seen);
      }
    },

    // Move an item on by the reply to its part: done, the next offset, or
    // stopped with a reason.
    applyReply(item: UploadItem, reply: PartReply | null, seen: { expected: number }) {
      if (reply?.status === "ok" && typeof reply.path === "string")
        return this.finished(item, reply.path);
      if (reply?.status === "ok" && typeof reply.received === "number") {
        item.sent = reply.received;
        return undefined;
      }
      const expected = resendFrom(reply, seen);
      if (expected !== null) {
        item.sent = expected;
        return undefined;
      }
      item.state = "failed";
      item.error = refusalText(reply);
      return undefined;
    },

    finished(item: UploadItem, path: string) {
      item.state = "done";
      item.sent = item.size;
      item.path = path;
      files.delete(item.key);
      if (path !== targetOf(item)) {
        messages.flash(`Uploaded as ${path}`);
      }
    },
  };
}

export type UploadsStore = ReturnType<typeof uploadsStore>;
