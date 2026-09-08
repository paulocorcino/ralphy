---
kind: fix
---
Several corrections to the release machinery, found by a review of it: the
changelog fold now refuses to re-fold a version whose fragments it already
consumed (which used to erase that release's record); `bump` refuses a version
Cargo would not accept instead of writing it into every manifest; a tag carrying
a non-ASCII character no longer aborts the version check; `ralphy update`
restarts the daemon onto the binary it just installed, and never ends a process
that only happens to share the old daemon's number.
