---
kind: breaking
---
Naming a Claude console is now something a project asks for. Turn on
`claude.console_name` — in Settings → Claude, or with
`ralphy config set claude.console_name true` — to keep opening consoles that
answer to `wb-<repo>-<hex>`; left off, Claude names each session itself, as it
did before the workbench started renaming them.
