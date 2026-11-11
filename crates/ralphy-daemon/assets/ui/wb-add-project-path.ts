// The text of a folder path in the Add a project dialog: its separator, its parts, how
// two paths compare, and a pasted WSL path.
//
// PURE, as `wb-add-project.ts`.

import type { State } from "./wb-add-project.ts";

/** A peer of `/api/fleet`, as far as this dialog reads it. */
export type Peer = {
  daemon_id: string;
  name: string;
  environment?: string;
  state?: string;
  diagnosis?: string;
};

// The separator of the daemon's paths, read from what it answered.
export function sepOf(state: State) {
  // Only a Windows daemon lists drives, with an empty folder path.
  if (state.listing?.dir?.path === "") return "\\";
  const sample = pathSample(state);
  return /^[A-Za-z]:/.test(sample) || sample.includes("\\") ? "\\" : "/";
}

// A path the daemon answered, or else the text in the field.
function pathSample(state: State) {
  const l = state.listing;
  return (l?.start ?? "") || (l?.dir?.path ?? "") || state.text;
}

function isSep(c: string) {
  return c === "/" || c === "\\";
}

export function endsWithSep(text: string) {
  const last = text.at(-1);
  return last !== undefined && isSep(last);
}

export function withSep(path: string, sep: string) {
  return endsWithSep(path) ? path : path + sep;
}

// `C:\Dev\..` and `/home/me/..\` go up one level, as in a shell.
export function collapseDots(text: string) {
  const m = /^(.*[\\/])[^\\/]+[\\/]\.\.([\\/]?)$/.exec(text);
  return m ? m[1] : text;
}

// A pasted `\\wsl.localhost\<distro>\…` or `\\wsl$\<distro>\…` path. `null`
// when the text is not one; else the peer's id and the Linux path, or the
// distro that has no peer.
export function mapWsl(text: string, peers: Peer[] | null | undefined) {
  const m = /^[\\/]{2}(?:wsl\.localhost|wsl\$)[\\/]([^\\/]+)([\\/].*)?$/i.exec(text || "");
  const distro = m?.[1];
  if (!m || distro === undefined) return null;
  const rest = ((m[2] ?? "") || "/").replaceAll("\\", "/");
  const peer = (peers ?? []).find((p) => {
    const named = /^WSL: (.+)$/iu.exec(p.environment ?? "")?.[1];
    return named !== undefined && named.toLowerCase() === distro.toLowerCase();
  });
  if (!peer) return { missing: distro };
  return { daemon: peer.daemon_id, path: rest };
}

// The folder name the text ends with, after the last separator.
export function lastName(text: string) {
  const i = Math.max(text.lastIndexOf("/"), text.lastIndexOf("\\"));
  return text.slice(i + 1);
}

export function parentText(text: string) {
  const i = Math.max(text.lastIndexOf("/"), text.lastIndexOf("\\"));
  return text.slice(0, i + 1);
}

export function basename(path: string) {
  return lastName(path.replace(/[\\/]+$/, "")) || path;
}

export function sameName(a: string, b: string, sep: string) {
  return sep === "\\" ? a.toLowerCase() === b.toLowerCase() : a === b;
}

export function samePath(a: string, b: string, sep: string) {
  const norm = (p: string) => {
    const s = (p || "").replace(/[\\/]+$/u, "").replaceAll("\\", "/");
    return sep === "\\" ? s.toLowerCase() : s;
  };
  return norm(a) === norm(b);
}
