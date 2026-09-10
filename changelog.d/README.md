# Changelog fragments

One file per pull request, named for the pull request or issue number
(`389.md`), or for the work when there is no number yet (`release-watch.md` —
it renders without a link rather than faking one).

```markdown
---
kind: feature
---
Paste a screenshot straight into a console: it lands as a file and the path is
what the agent receives.
```

`kind` is a closed set. It is the only severity the release machinery has while
the project ships candidates, so it decides three things at once: which heading
the entry lands under, whether the workbench badges the release loudly or
quietly, and whether the release is announced in Discussions at all
([ADR-0056](../docs/adr/0056-release-communication-and-the-update-watch.md)).

| `kind`     | Heading    | Announced | Use it when |
|------------|------------|-----------|-------------|
| `breaking` | Breaking   | yes       | an operator has to change something to keep working |
| `security` | Security   | yes       | a vulnerability was closed |
| `feature`  | New        | yes       | there is something a user can now do |
| `fix`      | Fixed      | no        | something stopped misbehaving |
| `internal` | —          | no        | nothing a user can see: a refactor, a test, a build change |

An `internal` fragment is consumed and never recorded. It exists so the author
can state, in the pull request, that the change says nothing to a user — which
is what lets the CI gate be strict about the other four.

## Writing the prose

Name the capability, not the diff. The commit subject says what changed; the
fragment says what the reader can now do. English, like every other artifact in
this repo, whatever language the request arrived in.

> Bad: *the virtual keyboard no longer paints over the prompt*
> — accurate, and useless to someone who never knew a keyboard was in the way.
>
> Good: *On a tablet, the on-screen keyboard no longer covers the console
> prompt you are typing into.*

**One sentence. Two, and short ones, when the first is useless without the
second — never more than two rendered lines.** The changelog is scanned, not
read: an entry that explains the mechanism, lists the states it handles or
recounts the bug is a paragraph the reader skips, and it hides the eleven
entries around it. The mechanism, the screenshot and the clip belong in the
pull request; the reader gets what they can now do.

> Bloated: *The workbench tells you when a newer build is out. A dot on the
> account puck, and a What's new panel listing every release between the one
> you are running and the newest, with what each one changed. A fixes-only
> release is a quiet dot; a breaking or security release stays on screen until
> you dismiss it. "Stop checking" turns the whole thing off.*
>
> Enough: *The workbench dots the account puck when a newer build is out, with
> a What's new panel listing what each release in the gap changed.*

## The fold

At release time the fragments are consumed:

```bash
cargo run -p xtask -- changelog --pending              # what the next release would say
cargo run -p xtask -- changelog --release v0.1.0-rc.20 # fold, and delete the fragments
```

That writes `CHANGELOG.md` and `changelog.json` at the repo root — both
machine-owned, never hand-edited — plus the release body and the announce flag
under `target/changelog/`.
