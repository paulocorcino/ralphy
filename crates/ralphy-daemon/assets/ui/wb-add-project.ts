// The Add a project dialog's state (issue #501, PRD #500, and the ADR-0036
// amendment "the registry verbs").
//
// PURE: state + event in, new state out. No DOM, no fetch, no Alpine. The shell
// sends `dir.list` and `project.add` and feeds the replies to `next`; the
// button label, the help line and the WSL path mapping come out of here.

import { WBFail } from "./wb-fail.ts";

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
  listing: any;
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

// The separator of the daemon's paths, read from what it answered.
function sepOf(state: State) {
  const l = state.listing;
  // Only a Windows daemon lists drives, with an empty folder path.
  if (l && l.dir && l.dir.path === "") return "\\";
  const sample = (l && (l.start || (l.dir && l.dir.path))) || state.text || "";
  return /^[A-Za-z]:/.test(sample) || sample.includes("\\") ? "\\" : "/";
}

function isSep(c: string) {
  return c === "/" || c === "\\";
}

function endsWithSep(text: string) {
  return text.length > 0 && isSep(text[text.length - 1]);
}

function withSep(path: string, sep: string) {
  return endsWithSep(path) ? path : path + sep;
}

// `C:\Dev\..` and `/home/me/..\` go up one level, as in a shell.
function collapseDots(text: string) {
  const m = /^(.*[\\/])[^\\/]+[\\/]\.\.([\\/]?)$/.exec(text);
  return m ? m[1] : text;
}

// A pasted `\\wsl.localhost\<distro>\…` or `\\wsl$\<distro>\…` path. `null`
// when the text is not one; else the peer's id and the Linux path, or the
// distro that has no peer.
function mapWsl(text: string, peers: any[] | null | undefined) {
  const m = /^[\\/]{2}(?:wsl\.localhost|wsl\$)[\\/]([^\\/]+)([\\/].*)?$/i.exec(text || "");
  if (!m) return null;
  const distro = m[1];
  const rest = (m[2] || "/").replace(/\\/g, "/");
  const peer = (peers || []).find((p) => {
    const env = /^WSL: (.+)$/i.exec(p.environment || "");
    return env && env[1].toLowerCase() === distro.toLowerCase();
  });
  if (!peer) return { missing: distro };
  return { daemon: peer.daemon_id, path: rest };
}

// The Where choices: this computer, then every peer. A peer that does not
// answer is shown but cannot be chosen.
function places(peers: any[] | null | undefined) {
  const list = [{ id: "", label: "This computer", disabled: false, reason: "" }];
  for (const p of peers || []) {
    const reachable = p.state === "reachable";
    list.push({
      id: p.daemon_id,
      label: p.environment ? `${p.name} (${p.environment})` : p.name,
      disabled: !reachable,
      reason: reachable ? "" : p.diagnosis || "This computer does not answer.",
    });
  }
  return list;
}

// What the next `dir.list` asks for.
function request(state: State) {
  if (state.needStart) return { daemon: state.daemon };
  return { daemon: state.daemon, path: state.text };
}

// The folder name the text ends with, after the last separator.
function lastName(text: string) {
  const i = Math.max(text.lastIndexOf("/"), text.lastIndexOf("\\"));
  return text.slice(i + 1);
}

function parentText(text: string) {
  const i = Math.max(text.lastIndexOf("/"), text.lastIndexOf("\\"));
  return text.slice(0, i + 1);
}

function basename(path: string) {
  return lastName(path.replace(/[\\/]+$/, "")) || path;
}

// The folders the daemon listed.
function listed(state: State) {
  return (state.listing && state.listing.entries) || [];
}

// The text that lists the folder above the listed one: its parent, or ""
// (the drive list) above a Windows drive root. `null` at the top.
function upText(state: State) {
  const dir = state.listing && state.listing.dir;
  if (!dir || !dir.path || !endsWithSep(state.text)) return null;
  const trimmed = dir.path.replace(/[\\/]+$/, "");
  if (!trimmed) return null;
  if (/^[A-Za-z]:$/.test(trimmed)) return "";
  return parentText(trimmed) || null;
}

// The rows of the list under the field: `..` first when there is a folder
// above, then the listed folders.
function entries(state: State) {
  const rows = listed(state);
  return upText(state) === null ? rows : [{ name: "..", up: true, repo: false, added: false }].concat(rows);
}

function sameName(a: string, b: string, sep: string) {
  return sep === "\\" ? a.toLowerCase() === b.toLowerCase() : a === b;
}

function samePath(a: string, b: string, sep: string) {
  const norm = (p: string) => {
    const s = (p || "").replace(/[\\/]+$/, "").replace(/\\/g, "/");
    return sep === "\\" ? s.toLowerCase() : s;
  };
  return norm(a) === norm(b);
}

// The folder the text names and what adding it would do:
// `{ action, label, help, path, init, create }`, where `action` is "add",
// "init", "create", "root", "added", "missing", "unreadable" or "none".
function target(state: State) {
  const sep = sepOf(state);
  const none = { action: "none", label: "Add project", help: "", path: "", init: false };
  if (state.wslMissing) return none;
  const text = state.text;
  if (!text || !state.chosen) return none;
  // A reply to an older text says nothing about this one.
  if (state.listedText !== text) return none;
  if (state.failure === "this folder does not exist") {
    return { action: "missing", label: "This folder does not exist", help: "", path: "", init: false };
  }
  const l = state.listing;
  if (!l || !l.dir || l.dir.path === "") return none;
  const dir = l.dir;
  let path;
  let repo;
  let added;
  let inside = null;
  if (endsWithSep(text)) {
    path = dir.path;
    repo = !!dir.root && samePath(dir.root, dir.path, sep);
    added = !!dir.added;
    if (dir.root && !repo) inside = dir.root;
  } else {
    const name = lastName(text);
    const entry = listed(state).find((e: any) => sameName(e.name, name, sep));
    // The parent folder exists: the last name can be created there. Not
    // inside a repo, where a new repository would sit in another one.
    if (!entry && !dir.root && name.trim()) {
      // Windows drops a final space or dot, so the folder would get
      // another name than the one shown.
      if (/[ .]$/.test(name)) {
        return { action: "missing", label: "A folder name cannot end with a space or a dot", help: "", path: "", init: false };
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
    if (!entry) {
      return { action: "missing", label: "This folder does not exist", help: "", path: "", init: false };
    }
    if (entry.error) {
      return { action: "unreadable", label: "Cannot read this folder", help: "", path: "", init: false };
    }
    path = withSep(dir.path, sep) + entry.name;
    repo = entry.repo;
    added = entry.added || (!entry.repo && !!dir.root && !!dir.added);
    if (!entry.repo && dir.root) inside = dir.root;
  }
  path = path.length > 3 ? path.replace(/[\\/]+$/, "") : path;
  if (added) return { action: "added", label: "Already in Projects", help: "", path, init: false };
  if (repo) return { action: "add", label: "Add project", help: "", path, init: false };
  if (inside) {
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
  const clickable = t.action === "add" || t.action === "root" || t.action === "init" || t.action === "create";
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
  if (t.create) payload.create = true;
  return payload;
}

function reset(state: State, patch: Partial<State>) {
  return Object.assign({}, state, {
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
  }, patch);
}

function next(state: State, ev: any) {
  switch (ev.type) {
    case "open":
      return reset(initial(), { open: true, daemon: ev.daemon || "" });
    case "close":
      return Object.assign({}, state, { open: false });
    case "where":
      if (state.adding) return state;
      return reset(state, { daemon: ev.daemon || "" });
    case "text": {
      if (state.adding) return state;
      const wsl = mapWsl(ev.text, ev.peers);
      if (wsl && wsl.missing) {
        return Object.assign({}, state, { text: ev.text, wslMissing: wsl.missing, active: -1, error: "" });
      }
      if (wsl) {
        return Object.assign({}, state, {
          daemon: wsl.daemon,
          text: wsl.path,
          needStart: false,
          chosen: true,
          listing: null,
          failure: "",
          wslMissing: "",
          active: -1,
          error: "",
        });
      }
      return Object.assign({}, state, {
        text: collapseDots(ev.text),
        needStart: false,
        chosen: true,
        wslMissing: "",
        active: -1,
        error: "",
      });
    }
    case "sent":
      return Object.assign({}, state, { seq: ev.seq, sentText: state.needStart ? null : state.text });
    case "slow":
      // A reply that came first means there is nothing to wait for.
      if (ev.seq !== state.seq || state.answered === ev.seq) return state;
      return Object.assign({}, state, { loading: true });
    case "reply": {
      if (ev.seq !== state.seq) return state;
      const reply = ev.reply || {};
      if (reply.status !== "ok") {
        return Object.assign({}, state, {
          answered: ev.seq,
          listing: null,
          listedText: state.sentText,
          failure: reply.message || reply.reason || "the folder list failed",
          loading: false,
          active: -1,
        });
      }
      const patch: Partial<State> = {
        answered: ev.seq,
        listing: reply,
        listedText: state.sentText,
        failure: "",
        loading: false,
        active: -1,
      };
      if (state.sentText === null && reply.start) {
        const sep = /^[A-Za-z]:/.test(reply.start) || reply.start.includes("\\") ? "\\" : "/";
        patch.text = withSep(reply.start, sep);
        patch.listedText = patch.text;
        patch.needStart = false;
      }
      return Object.assign({}, state, patch);
    }
    case "move": {
      const n = entries(state).length;
      if (!n) return state;
      const at = state.active < 0 ? (ev.by > 0 ? 0 : n - 1) : (state.active + ev.by + n) % n;
      return Object.assign({}, state, { active: at });
    }
    case "pick": {
      if (state.adding) return state;
      const sep = sepOf(state);
      if (ev.up) {
        const up = upText(state);
        if (up === null) return state;
        return Object.assign({}, state, { text: up, needStart: false, chosen: true, active: -1, error: "" });
      }
      // The rows on screen belong to the listed folder, not to the text: a
      // pick made before the next listing arrives must not add its name to
      // the folder the previous pick already chose.
      const dir = state.listing && state.listing.dir;
      const base = dir ? withSep(dir.path, sep) : endsWithSep(state.text) ? state.text : parentText(state.text);
      const text = dir && dir.path === "" ? ev.name : base + ev.name;
      return Object.assign({}, state, { text: withSep(text, sep), needStart: false, chosen: true, active: -1, error: "" });
    }
    case "adding":
      return Object.assign({}, state, { adding: true, error: "" });
    case "added":
      return Object.assign({}, state, { adding: false, open: false });
    case "addFailed":
      return Object.assign({}, state, { adding: false, error: ev.message || "The project was not added." });
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
