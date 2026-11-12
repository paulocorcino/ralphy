// The folder list of the Add a project dialog: the rows the daemon listed, and the folder above.
//
// PURE, as `wb-add-project.ts`.

import type { Row, State } from "./wb-add-project.ts";
import { endsWithSep, parentText } from "./wb-add-project-path.ts";

// The folders the daemon listed.
export function listed(state: State): Row[] {
  return state.listing?.entries ?? [];
}

// The text that lists the folder above the listed one: its parent, or ""
// (the drive list) above a Windows drive root. `null` at the top.
export function upText(state: State) {
  const dir = state.listing && state.listing.dir;
  if (!dir || !dir.path || !endsWithSep(state.text)) return null;
  const trimmed = dir.path.replace(/[\\/]+$/u, "");
  if (!trimmed) return null;
  if (/^[A-Za-z]:$/u.test(trimmed)) return "";
  return parentText(trimmed) || null;
}

// The rows of the list under the field: `..` first when there is a folder
// above, then the listed folders.
export function entries(state: State) {
  const rows = listed(state);
  return upText(state) === null
    ? rows
    : ([{ name: "..", up: true, repo: false, added: false }] as Row[]).concat(rows);
}

// The folder the typed text names: the text itself when it ends with a separator.
export function typedFolder(text: string) {
  return endsWithSep(text) ? text : parentText(text);
}
