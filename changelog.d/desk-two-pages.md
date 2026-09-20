---
kind: fix
---
A console no longer comes back twice when the workbench is open on more than
one device: a page merges the desk with what the other pages saved before it
writes, and a session whose record lost its id returns to its own window
instead of a new one beside it.
