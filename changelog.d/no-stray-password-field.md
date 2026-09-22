---
kind: fix
---
The workbench no longer keeps a password field in the page when nothing is
asking for a password: the login gate's and the settings modal's are rendered
only while those surfaces are open. Typing in any other field — renaming a note,
for one — stopped making the browser offer to save what you typed as a password.
