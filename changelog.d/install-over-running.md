---
kind: fix
---
`ralphy install --force` can now replace a `ralphy` that is currently running —
the resident daemon being the ordinary case. It moves the old binary aside
instead of deleting it, which is what Windows allows, and puts it back if the
replacement fails.
