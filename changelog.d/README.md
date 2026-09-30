# Changelog fragments

One file per pull request, named for the pull request or issue number
(`389.md`), or for the work when there is no number yet (`release-watch.md`).

```markdown
---
kind: feature
headline: 📝 **Notes** — markdown notes on the consoles stage
---
Write markdown notes on the consoles stage, saved as `.note` files in the project.
```

## `kind`

| `kind`     | Heading  | Announced | Use it when |
|------------|----------|-----------|-------------|
| `breaking` | Breaking | yes       | an operator has to change something to keep working |
| `security` | Security | yes       | a vulnerability was closed |
| `feature`  | New      | yes       | there is something a user can now do |
| `fix`      | Fixed    | no        | something that a release shipped stopped misbehaving |
| `internal` | —        | no        | nothing a released build can show: a refactor, a test, a build change, or a fix to a feature no release has shipped yet |

`kind` also decides how loudly the workbench badges the release and whether it
is announced in Discussions
([ADR-0056](../docs/adr/0056-release-communication-and-the-update-watch.md)).
An `internal` fragment is recorded nowhere.

**A fix to an unreleased feature is `internal`.** No user had the bug, so the
fix is part of shipping the feature. Check with
`git merge-base --is-ancestor <feature-commit> <last-tag>`: if the feature is
not in the last tag, the fix is `internal`.

## The prose

**One line: what the user can now do, and where.** The check refuses prose
over 140 characters and a headline over 100. The mechanism, the states it
handles, and the bug story go in the pull request.

> Too long: *A new button in the fence title bar opens all of the fence's
> consoles as columns. The grid follows their places on the stage: consoles
> side by side become columns, and consoles one above the other become rows of
> one column. Restore each console to put it back where it was.*
>
> Enough: *A button in the fence title bar opens all of its consoles as
> columns.*

Name the capability, not the diff:

> Diff: *the virtual keyboard no longer paints over the prompt*
>
> Capability: *On a tablet, the on-screen keyboard no longer covers the
> console prompt.*

Plain English for a reader who is not a native speaker: common words, short
subject–verb–object sentences, the feature's name first, no idioms (*reads at
a glance*, *for free*), and no names of Ralphy's own tooling.

## The page

`CHANGELOG.md` gets every fragment's prose. The release page and the
workbench's What's new panel get one line per capability: the `headline`, or
the prose's first sentence.

`topic:` is a slug that joins several `feature` fragments of one capability
into one line on the page. Give the main fragment the headline and the same
topic to the rest.

## The fold

```bash
cargo run -p xtask -- changelog --pending              # what the next release would say
cargo run -p xtask -- changelog --release v0.1.0-rc.20 # fold, and delete the fragments
```

That writes `CHANGELOG.md` and `changelog.json` (machine-owned, never
hand-edited) plus the release body and the announce flag under
`target/changelog/`.
