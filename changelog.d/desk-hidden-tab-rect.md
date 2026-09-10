---
kind: fix
---
Reloading the workbench from any tab other than Consoles no longer moves every
restored console to the top-left corner. A window restored while its tab was
hidden measured 0×0 at 0,0, and that box was saved over its real place; the
saved layout now keeps the window's box until it is actually moved or resized.
