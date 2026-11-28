/* ---------------------------------------------------------------------------
   The texts and locks of the Changes view that the markup shows: the bodies
   of members of `wbChanges` (wb-changes-panel.ts), which takes the component
   as `view`.
   --------------------------------------------------------------------------- */
import type { AlpineMagics } from "./wb-alpine.ts";
import type { ChangeEntry } from "./wb-shell-types.d.ts";

/** What these members read on the Changes component. */
type ChangesView = {
  $store: AlpineMagics["$store"];
  syncBusy: "fetch" | "pull" | "push" | null;
  commitMsg: string;
  commitMsgSlug: string | null;
  changesStaged: Record<string, ChangeEntry[]>;
  commitTarget(): { label: string };
  syncBusyTitle(verb: string): string;
  pullBlocked(): string;
  pushAct(): { title: string };
};

const ACTING: Record<string, string | undefined> = {
  fetch: "Fetching…",
  pull: "Pulling…",
  push: "Pushing…",
};

// The busy act names itself, the other two name what they are waiting on.
export function busyTitle(view: ChangesView, verb: string) {
  if (view.syncBusy === null) return "";
  if (view.syncBusy !== verb) return `Waiting for the ${view.syncBusy} to finish`;
  return ACTING[verb] ?? "";
}

export function fetchTitle(view: ChangesView) {
  return (
    view.syncBusyTitle("fetch") ||
    view.$store.connection.writeLockReason() ||
    "Fetch from the upstream"
  );
}

export function pullTitle(view: ChangesView) {
  return (
    view.syncBusyTitle("pull") ||
    view.pullBlocked() ||
    view.$store.connection.writeLockReason() ||
    "Pull from the upstream (fast-forward only)"
  );
}

export function pullLocked(view: ChangesView) {
  return (
    view.$store.connection.writeLocked() || view.syncBusy !== null || view.pullBlocked() !== ""
  );
}

function stagedCount(view: ChangesView) {
  return (view.changesStaged[view.$store.projects.openKey()] ?? []).length;
}

export function commitTitle(view: ChangesView) {
  const locked = view.$store.connection.writeLockReason();
  if (locked) return locked;
  if (stagedCount(view) === 0) return "Stage a change first";
  if (!view.commitMsg.trim()) return "Write a commit message first";
  return view.commitTarget().label;
}

export function canCommit(view: ChangesView) {
  return (
    !view.$store.connection.writeLocked() &&
    view.commitMsgSlug === view.$store.projects.openSlug &&
    view.commitMsg.trim() !== "" &&
    stagedCount(view) > 0
  );
}

// Why the acts wait, as a line on screen: a touch screen shows no `title`.
export function holdText(view: ChangesView) {
  const locked = view.$store.connection.writeLockReason();
  if (locked) return locked;
  if (view.commitMsg.trim() !== "" && stagedCount(view) === 0) {
    return "Stage a change first, then commit.";
  }
  return "";
}

export function rowActTitle(view: ChangesView, verb: string, all: boolean) {
  const locked = view.$store.connection.writeLockReason();
  if (locked) return locked;
  if (verb === "stage") return all ? "Stage all changes" : "Stage changes";
  if (verb === "discard") return "Discard changes";
  return all ? "Unstage all changes" : "Unstage changes";
}

// Push's title (#320) states the run-lock reason, as `rowActTitle` does.
export function pushTitle(view: ChangesView) {
  return (
    view.syncBusyTitle("push") || view.$store.connection.writeLockReason() || view.pushAct().title
  );
}
