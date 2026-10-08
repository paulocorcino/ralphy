---
kind: fix
---
The status and stop hooks read their input before they exit, so a vendor never gets a broken pipe.
