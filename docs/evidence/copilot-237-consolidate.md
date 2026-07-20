# #237 live one-shot smoke: `ralphy consolidate --agent copilot`

Exercises the new `build_copilot_init_command` builder, the D8 env scrub, and
`run_text_session` against a real `copilot` process — the runtime leg the
static tests in `command.rs`/`tasks.rs` cannot give.

## Setup

Scratch repo `/tmp/ralphy-copilot-237` (`git init`, one committed `README.md`),
with `.ralphy/knowledge/KNOWLEDGE.md` carrying one `## Traps` heading, one
bullet with a `(#1 …)` provenance marker, a trailing `<!-- folded: none -->`
line, and a loose note `.ralphy/knowledge/issue-1.md`.

## Command

```
env -u GH_TOKEN -u GITHUB_TOKEN -u COPILOT_GITHUB_TOKEN \
    ./target/debug/ralphy.exe consolidate --repo /tmp/ralphy-copilot-237 \
    --agent copilot --max-minutes 15
```

## Output

```
Consolidating 1 note(s) into KNOWLEDGE.md: issue-1.md
Done: KNOWLEDGE.md updated, 1 note(s) archived into .ralphy/knowledge/raw/.
```

## Verified

- stdout contains the literal `Done: KNOWLEDGE.md updated,`.
- `/tmp/ralphy-copilot-237/.ralphy/knowledge/raw/issue-1.md` exists after the
  run — the note was archived, not left loose.
- `KNOWLEDGE.md` was rewritten by the `copilot` session with the note's fact
  folded in and a `<!-- folded: #1 -->` marker.
