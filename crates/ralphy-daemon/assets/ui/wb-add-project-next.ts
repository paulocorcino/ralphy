// The folds of the Add a project dialog that a text, a folder list, an arrow or a pick starts.
//
// PURE, as `wb-add-project.ts`.

import type { State, Step } from "./wb-add-project.ts";
import { collapseDots, mapWsl, sepOf, withSep } from "./wb-add-project-path.ts";
import { entries, typedFolder, upText } from "./wb-add-project-list.ts";

// The operator typed: a pasted WSL path moves to that peer, else the text is
// kept as typed.
export function nextText(state: State, ev: Extract<Step, { type: "text" }>) {
  if (state.adding) return state;
  const wsl = mapWsl(ev.text, ev.peers);
  if (wsl && "missing" in wsl) {
    return Object.assign({}, state, {
      text: ev.text,
      wslMissing: wsl.missing,
      active: -1,
      error: "",
    });
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

// Why a folder list failed, in the daemon's words when it gave any.
function listFailure(reply: { message?: string; reason?: string }) {
  return (reply.message ?? "") || (reply.reason ?? "") || "the folder list failed";
}

// The daemon answered a folder list: a newer request keeps the state, a
// failure keeps the failure, and the start folder fills a field the operator
// has not touched.
export function nextReply(state: State, ev: Extract<Step, { type: "reply" }>) {
  if (ev.seq !== state.seq) return state;
  const reply = ev.reply;
  if (reply.status !== "ok") {
    return Object.assign({}, state, {
      answered: ev.seq,
      listing: null,
      listedText: state.sentText,
      failure: listFailure(reply),
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
  if (state.sentText === null && reply.start !== undefined && reply.start !== "") {
    const sep = /^[A-Za-z]:/u.test(reply.start) || reply.start.includes("\\") ? "\\" : "/";
    patch.text = withSep(reply.start, sep);
    patch.listedText = patch.text;
    patch.needStart = false;
  }
  return Object.assign({}, state, patch);
}

// The arrows: the highlight moves by one row and wraps.
export function nextMove(state: State, ev: Extract<Step, { type: "move" }>) {
  const n = entries(state).length;
  if (!n) return state;
  const edge = ev.by > 0 ? 0 : n - 1;
  const at = state.active < 0 ? edge : (state.active + ev.by + n) % n;
  return Object.assign({}, state, { active: at });
}

// A row was picked: `..` goes up, a folder goes down one level.
export function nextPick(state: State, ev: Extract<Step, { type: "pick" }>) {
  if (state.adding) return state;
  const sep = sepOf(state);
  if (ev.up === true) {
    const up = upText(state);
    if (up === null) return state;
    return Object.assign({}, state, {
      text: up,
      needStart: false,
      chosen: true,
      active: -1,
      error: "",
    });
  }
  // The rows on screen belong to the listed folder, not to the text: a
  // pick made before the next listing arrives must not add its name to
  // the folder the previous pick already chose.
  const dir = state.listing && state.listing.dir;
  const base = dir ? withSep(dir.path, sep) : typedFolder(state.text);
  const text = dir && dir.path === "" ? ev.name : base + ev.name;
  return Object.assign({}, state, {
    text: withSep(text, sep),
    needStart: false,
    chosen: true,
    active: -1,
    error: "",
  });
}
