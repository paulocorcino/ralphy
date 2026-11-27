/* ---------------------------------------------------------------------------
   The `delete` gesture of the file tree: one confirmation for every entry it
   names, then one `file.delete` per entry, in order. A folder goes with its
   contents.
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { parentRel } from "./wb-file-paths.ts";
import { sendWindow } from "./wb-events.ts";
import type { Messages } from "./wb-messages.ts";

type DeleteAction = Extract<WorkbenchAction, { action: "delete" }>;
type Entry = DeleteAction["entries"][number];

/** Where the deletes are aimed (`actionTarget` of wb-wire.ts). */
type DeleteAim = { repo: string; aimed: (payload: CommandPayload) => CommandPayload };

/** The outcome of one `file.delete`: null when it worked. */
type Refusal = { entry: Entry; text: string } | null;

const nameOf = (e: Entry) => e.title || (e.path.split("/").pop() ?? "") || e.path;

function question(entries: Entry[]): string {
  const [only] = entries;
  if (entries.length === 1 && only) {
    return only.isFolder
      ? `Delete folder “${nameOf(only)}” and its contents? This cannot be undone.`
      : `Delete “${nameOf(only)}”? This cannot be undone.`;
  }
  const folders = entries.some((e) => e.isFolder) ? " Each folder goes with its contents." : "";
  return `Delete ${entries.length} items?${folders} This cannot be undone.`;
}

async function confirmed(messages: Messages, entries: Entry[]): Promise<boolean> {
  const message = question(entries);
  const ask = messages.askInShell({
    title: "Delete",
    message,
    confirmLabel: "Delete",
    danger: true,
  });
  const ok = ask ? await ask : window.confirm(message);
  return ok === true;
}

async function deleteOne(entry: Entry, { repo, aimed }: DeleteAim): Promise<Refusal> {
  const reply = await WBDaemon.write("file.delete", aimed({ repo, path: entry.path })).catch(
    () => null,
  );
  if (!reply) return { entry, text: "Could not delete: the daemon did not answer." };
  if (!WBFail.isError(reply)) return null;
  // "not found" says the ROW is the lie: re-list the parent so the ghost
  // ends up off the screen.
  if (/not found/iu.test(WBFail.message(reply, "the daemon gave no reason"))) {
    sendWindow(window, "workbench:tree-dirty", { rel: parentRel(entry.path) });
  }
  return { entry, text: WBFail.failed(reply, "Could not delete: the daemon gave no reason.") };
}

function report(entries: Entry[], refusals: Refusal[]): string {
  const failed = refusals.filter((r) => r !== null);
  const [first] = failed;
  if (!first) {
    const [only] = entries;
    return entries.length === 1 && only
      ? `${nameOf(only)} deleted.`
      : `${entries.length} items deleted.`;
  }
  if (entries.length === 1) return first.text;
  const done = entries.length - failed.length;
  return `${done} of ${entries.length} items deleted. ${nameOf(first.entry)}: ${first.text}`;
}

/** Ask once, delete each entry, and flash one line for the whole gesture. */
export async function deleteEntries(
  d: DeleteAction,
  aim: DeleteAim,
  messages: Messages,
): Promise<void> {
  if (d.entries.length === 0 || !(await confirmed(messages, d.entries))) return;
  const refusals: Refusal[] = [];
  for (const entry of d.entries) refusals.push(await deleteOne(entry, aim));
  messages.flash(report(d.entries, refusals));
}
