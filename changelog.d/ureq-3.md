---
kind: internal
---
The release watch, `ralphy update`, the price table fetch, the event sink and the
Telegram notifier now use ureq 3. They still ignore proxy environment variables,
as before, and a test keeps it that way.
