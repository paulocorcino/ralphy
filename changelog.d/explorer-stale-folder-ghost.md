---
kind: fix
---
The Explorer no longer shows a folder that isn't there. Reusing a directory
name — renaming `ideias/` away and creating a fresh `ideias/` — used to make the
new folder inherit the old one's contents from the browser's tree memory, so it
listed children it never had; expanding one answered "not found" and deleting it
did nothing at all. A directory listing now evicts what it contradicts, and a
delete that comes back "not found" re-lists the parent, so the row goes away
either way.
