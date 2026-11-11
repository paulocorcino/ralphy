---
kind: internal
---
The Rust lint gate now fails on a value dropped by `let _ =` with no type, or by `mem::forget`.
