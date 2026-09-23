---
kind: internal
---
The daemon can read and write a note: `note.read`/`note.write` carry markdown
in and out of the opaque `.note` container, landing by default in
`.ralphy/notes/` — the one place the workbench's write denylist now opens, so
a note can also be renamed and deleted from the explorer. Nothing in the
workbench uses them yet; the card arrives in a later slice.
