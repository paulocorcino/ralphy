# Documentation

Start with the task you need to perform. Guides describe supported behavior;
ADRs record decisions; research and evidence record observations made at a
particular time.

## Use Ralphy

| Task | Document |
|---|---|
| Set up a project and start a run | [Getting started](getting-started.md) |
| Choose the planner and executor | [Agents](agents.md) |
| Set run flags and persistent defaults | [Run options](run-options.md), [configuration](configuration.md) |
| Use the workbench and manage daemons | [Daemon](daemon.md) |
| Schedule runs | [Scheduling](scheduling.md) |
| Configure notifications | [Telegram](telegram.md) |
| Understand usage and cost reports | [Usage and cost](usage-and-cost.md) |
| Organize the queue and verify delivery | [Triage roles](triage-roles.md), [verify gate](verify-gate.md) |
| Consume run events | [Event contract](events.md) |

## Develop Ralphy

| Task | Document |
|---|---|
| Follow repository rules | [Agent guide](../AGENTS.md) |
| Use the domain vocabulary | [Domain model](../CONTEXT.md) |
| Build, run CI checks, or release | [Building](BUILDING.md) |
| Write and review tests | [Testing](TESTING.md) |
| Understand architectural decisions | [ADRs](adr/README.md) |
| Change the workbench shell | [Workbench build guide](WORKBENCH-BUILD-GUIDE.md) |
| Exercise the initial daemon workflows manually | [Daemon bench](daemon-bench.md) (Phase 1 scope) |
| Find execution plans and unresolved follow-up work | [Plans](plans/README.md) |
| Find measurements and acceptance records | [Evidence](evidence/README.md) |
| Find design investigations and source material | [Research](research/README.md) |

## Where documents belong

- Keep user guides and the build/test guides at their existing paths in this
  directory. `BUILDING.md` is also packaged with releases.
- Keep decisions and their validation companions in `adr/`, following its
  numbering convention. Age does not make a decision obsolete.
- Put dated audits in `evidence/audits/` and acceptance records in `evidence/`.
  Label observations by date; do not present them as current test results.
  The evidence index also identifies older records that keep stable root
  paths for existing consumers.
- Put investigations in `research/`. Keep a source document when a retained
  analysis depends on it. Editorial material belongs in `essays/`; it is not
  the product contract.
- Keep execution plans, reports and their verification scripts together in
  `plans/`. A completed plan can still support unresolved follow-up work.
- `live/` holds raw captures used by validation records. A reference to a log
  family protects the whole family, even without individual filename links.
- `screenshots/` holds product images and historical captures. Repository ignore
  rules apply to new files; they do not remove previously tracked images.
- `ui-copy-rules.json` is executable lint configuration. Its location is part
  of the tooling contract, not a documentation organization choice.

## Retirement policy

`_retirement/` is a review area for possible deletion, not an archive of
required evidence. Its own README records each original path, reason and
deletion condition. No files are deleted as part of moving them there.

Before a move, check links, plain-text filenames, directory and wildcard
references, code, scripts, build configuration and local research. If retained
material still needs a document, keep it outside that directory. Move useful
information to its maintained owner before retiring the source. Repair relative
links from moved files as well as incoming references.

Deletion requires a separate review of the retirement manifest and unresolved
work. External issue links may still refer to old paths; Git history preserves
the original revisions. Do not treat missing local backlinks as proof that a
document has no value.

Ignored local reports, logs and scratch files are not published documentation.
Do not add or move them implicitly during a documentation cleanup. A maintained
document must contain the necessary conclusion or cite an accessible source,
rather than depend on an ignored file.
