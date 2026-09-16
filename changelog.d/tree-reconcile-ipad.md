---
kind: fix
---
The file tree no longer paints its rows a third of the way down a blank panel
after an agent writes files, and a folder refreshed while its parent is being
re-listed no longer reports "Could not refresh the file list (null is not an
object …)". A screenshot paste that the daemon drops without a reply now says
why, on both the console and the daemon log.
