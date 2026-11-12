// The Add a project dialog's state (issue #501, PRD #500, and the ADR-0036
// amendment "the registry verbs").
//
// PURE: state + event in, new state out. No DOM, no fetch, no Alpine. The shell
// sends `dir.list` and `project.add` and feeds the replies to `next`; the
// button label, the help line and the WSL path mapping come out of here.

import { WBFail } from "./wb-fail.ts";
import type { Peer } from "./wb-add-project-path.ts";
import {
  basename,
  endsWithSep,
  lastName,
  mapWsl,
  sameName,
  samePath,
  sepOf,
  withSep,
} from "./wb-add-project-path.ts";
import { entries, listed } from "./wb-add-project-list.ts";
import { nextMove, nextPick, nextReply, nextText } from "./wb-add-project-next.ts";

/** What the folder the text names would do when it is added. */
type Target = {
  action: string;
  label: string;
  help: string;
  path: string;
  init: boolean;
  create?: boolean;
};

/** The newest `dir.list` reply, as the daemon sent it. */
export type Listing = DaemonReplies["dir.list"];

/** A row of the folder list: a listed folder, or the `..` row that goes up. */
export type Row = { name: string; repo: boolean; added: boolean; error?: string; up?: boolean };

/** What the dialog tells the fold. */
export type Step =
  | { type: "open"; daemon?: string }
  | { type: "close" }
  | { type: "where"; daemon?: string }
  | { type: "text"; text: string; peers?: Peer[] }
  | { type: "sent"; seq: number }
  | { type: "slow"; seq: number }
  | { type: "reply"; seq: number; reply: Listing }
  | { type: "move"; by: number }
  | { type: "pick"; name: string; up?: boolean }
  | { type: "adding" }
  | { type: "added" }
  | { type: "addFailed"; message?: string };

/** The Add a project dialog's state. */
export type State = {
  open: boolean;
  daemon: string;
  text: string;
  needStart: boolean;
  chosen: boolean;
  seq: number;
  answered: number;
  /** The newest `dir.list` reply, as the daemon sent it. */
  listing: Listing | null;
  listedText: string | null;
  sentText: string | null;
  failure: string;
  loading: boolean;
  active: number;
  wslMissing: string;
  adding: boolean;
  error: string;
};

function initial(): State {
  return {
    open: false,
    // The daemon id the folder lives on; "" is the daemon this page is on.
    daemon: "",
    text: "",
    // The next listing asks for no path, so the daemon answers `start`.
    needStart: true,
    // The operator typed or picked a folder. The start folder the daemon
    // fills in is never a choice by itself: one Enter must not run
    // `git init` in a home folder.
    chosen: false,
    // The sequence number of the newest `dir.list` sent. Older replies are
    // dropped.
    seq: 0,
    // The sequence number of the newest reply that arrived.
    answered: 0,
    // The newest reply, and the text it answers.
    listing: null,
    listedText: null,
    // The text of the newest `dir.list` sent; `null` when it asked for start.
    sentText: null,
    // The newest `dir.list` refusal, or "".
    failure: "",
    loading: false,
    // Index of the highlighted folder in the list, or -1.
    active: -1,
    // Set when a pasted WSL path has no peer.
    wslMissing: "",
    adding: false,
    error: "",
  };
}

// The Where choices: this computer, then every peer. A peer that does not
// answer is shown but cannot be chosen.
function places(peers: Peer[] | null | undefined) {
  const list = [{ id: "", label: "This computer", disabled: false, reason: "" }];
  for (const p of peers ?? []) {
    const reachable = p.state === "reachable";
    list.push({
      id: p.daemon_id,
      label: (p.environment ?? "") === "" ? p.name : `${p.name} (${p.environment})`,
      disabled: !reachable,
      reason: reachable ? "" : (p.diagnosis ?? "") || "This computer does not answer.",
    });
  }
  return list;
}

// What the next `dir.list` asks for.
function request(state: State) {
  if (state.needStart) return { daemon: state.daemon };
  return { daemon: state.daemon, path: state.text };
}

// The folder the text names and what adding it would do:
// `{ action, label, help, path, init, create }`, where `action` is "add",
// "init", "create", "root", "added", "missing", "unreadable" or "none".
function target(state: State): Target {
  const none = { action: "none", label: "Add project", help: "", path: "", init: false };
  if (!answersText(state)) return none;
  if (state.failure === "this folder does not exist") {
    return {
      action: "missing",
      label: "This folder does not exist",
      help: "",
      path: "",
      init: false,
    };
  }
  const dir = state.listing?.dir;
  if (!dir || dir.path === "") return none;
  const text = state.text;
  const sep = sepOf(state);
  if (!endsWithSep(text)) return entryTarget(state, dir, text, sep);
  return folderTarget(dir, sep);
}

// The folder `dir` itself, named by a text that ends with a separator.
function folderTarget(dir: ListedDir, sep: string): Target {
  const root = dir.root ?? "";
  const repo = root !== "" && samePath(root, dir.path, sep);
  return finalTarget(dir.path, repo, dir.added, root !== "" && !repo ? root : null);
}

// The newest listing answers the text in the field, for a chosen place with
// no missing WSL peer. A reply to an older text says nothing about this one.
function answersText(state: State) {
  if (state.wslMissing || !state.text || !state.chosen) return false;
  return state.listedText === state.text;
}

/** The folder `dir` of a listing. */
type ListedDir = NonNullable<Listing["dir"]>;

// The text names a folder by its last name: a listed folder, or one that the
// parent folder `dir` can create.
function entryTarget(state: State, dir: ListedDir, text: string, sep: string): Target {
  const name = lastName(text);
  const entry = listed(state).find((e) => sameName(e.name, name, sep));
  // The parent folder exists: the last name can be created there. Not
  // inside a repo, where a new repository would sit in another one.
  if (!entry && (dir.root ?? "") === "" && name.trim()) return createTarget(dir, name, sep);
  if (!entry) {
    return {
      action: "missing",
      label: "This folder does not exist",
      help: "",
      path: "",
      init: false,
    };
  }
  return listedTarget(dir, entry, sep);
}

// A folder the daemon listed in the folder `dir`.
function listedTarget(dir: ListedDir, entry: Row, sep: string): Target {
  if ((entry.error ?? "") !== "") {
    return {
      action: "unreadable",
      label: "Cannot read this folder",
      help: "",
      path: "",
      init: false,
    };
  }
  const root = dir.root ?? "";
  const added = entry.added || (!entry.repo && root !== "" && dir.added);
  const inside = !entry.repo && root !== "" ? root : null;
  return finalTarget(withSep(dir.path, sep) + entry.name, entry.repo, added, inside);
}

// A folder `name` that does not exist yet in the folder `dir`.
function createTarget(dir: ListedDir, name: string, sep: string): Target {
  // Windows drops a final space or dot, so the folder would get
  // another name than the one shown.
  if (/[ .]$/u.test(name)) {
    return {
      action: "missing",
      label: "A folder name cannot end with a space or a dot",
      help: "",
      path: "",
      init: false,
    };
  }
  const path = withSep(dir.path, sep) + name;
  return {
    action: "create",
    label: "Create folder and repository",
    help: `Ralphy creates the folder ${path} and a git repository in it.`,
    path,
    init: true,
    create: true,
  };
}

// What adding the existing folder at `path` does: it is in Projects, it is a
// repo, it is inside the repo `inside`, or it gets a repository.
function finalTarget(given: string, repo: boolean, added: boolean, inside: string | null): Target {
  const path = given.length > 3 ? given.replace(/[\\/]+$/u, "") : given;
  if (added) return { action: "added", label: "Already in Projects", help: "", path, init: false };
  if (repo) return { action: "add", label: "Add project", help: "", path, init: false };
  if (inside !== null) {
    return {
      action: "root",
      label: `Add ${basename(inside)}`,
      help: `Part of ${basename(inside)}. Ralphy adds all of ${basename(inside)}.`,
      path,
      init: false,
    };
  }
  return {
    action: "init",
    label: "Create repository and add",
    help: "Ralphy creates a git repository here.",
    path,
    init: true,
  };
}

// The main button: its label, and whether it can be clicked.
function primary(state: State) {
  const t = target(state);
  const clickable =
    t.action === "add" || t.action === "root" || t.action === "init" || t.action === "create";
  return { label: state.adding ? "Adding…" : t.label, disabled: !clickable || state.adding };
}

// False when the chosen daemon cannot list folders at all. Only a peer can
// answer this: it runs a Ralphy older than the page's daemon, without the
// folder list. The Folder field is then hidden: nothing typed there works.
function listable(state: State) {
  return state.failure !== "unknown verb";
}

// The line under the field.
function help(state: State) {
  if (state.wslMissing) {
    return `Add ${state.wslMissing} (WSL) as a host first.`;
  }
  if (!listable(state)) {
    return "Update Ralphy on this computer to see its folders.";
  }
  if (state.failure && state.failure !== "this folder does not exist") {
    return WBFail.cause({ message: state.failure }, "Could not list this folder.");
  }
  return target(state).help;
}

// Enter adds what the main button adds, except a new folder: a typing error
// must not create one, so only a click does.
function enterAdds(state: State) {
  return !primary(state).disabled && target(state).action !== "create";
}

// What `project.add` sends.
function addPayload(state: State) {
  const t = target(state);
  const payload: { daemon: string; path: string; init: boolean; create?: boolean } = {
    daemon: state.daemon,
    path: t.path,
    init: t.init,
  };
  if (t.create === true) payload.create = true;
  return payload;
}

function reset(state: State, patch: Partial<State>) {
  return Object.assign(
    {},
    state,
    {
      text: "",
      needStart: true,
      chosen: false,
      listing: null,
      listedText: null,
      sentText: null,
      failure: "",
      loading: false,
      active: -1,
      wslMissing: "",
      error: "",
    },
    patch,
  );
}

function next(state: State, ev: Step) {
  switch (ev.type) {
    case "open":
      return reset(initial(), { open: true, daemon: ev.daemon ?? "" });
    case "close":
      return Object.assign({}, state, { open: false });
    case "where":
      if (state.adding) return state;
      return reset(state, { daemon: ev.daemon ?? "" });
    case "text":
      return nextText(state, ev);
    case "sent":
      return Object.assign({}, state, {
        seq: ev.seq,
        sentText: state.needStart ? null : state.text,
      });
    case "slow":
      // A reply that came first means there is nothing to wait for.
      if (ev.seq !== state.seq || state.answered === ev.seq) return state;
      return Object.assign({}, state, { loading: true });
    case "reply":
      return nextReply(state, ev);
    case "move":
      return nextMove(state, ev);
    case "pick":
      return nextPick(state, ev);
    case "adding":
      return Object.assign({}, state, { adding: true, error: "" });
    case "added":
      return Object.assign({}, state, { adding: false, open: false });
    case "addFailed":
      return Object.assign({}, state, {
        adding: false,
        error: (ev.message ?? "") || "The project was not added.",
      });
    default:
      return state;
  }
}

export const WBAddProject = {
  initial: initial,
  next: next,
  request: request,
  entries: entries,
  target: target,
  primary: primary,
  help: help,
  listable: listable,
  addPayload: addPayload,
  enterAdds: enterAdds,
  places: places,
  mapWsl: mapWsl,
  sepOf: sepOf,
};
