# Hand-off prompt template

Each stage's hand-off prompt MUST include the following. Replace `{placeholders}`; keep the structure.

The four generic sections (Authorization, Scope discipline, Failure protocol, Return to parent) are **not repeated here** — they live once in `## Hand-off conventions` in the plan, and the hand-off references them by name. The subagent reads the plan end-to-end before executing, so the conventions are always in scope.

```
You are executing Stage {N} of {plan title} at {absolute plan path}.
Read that plan file end-to-end once for context, then read {repo}/CLAUDE.md for repo-wide rules.
Your authoritative spec is the block between `<!-- BEGIN STAGE {N} -->` and `<!-- END STAGE {N} -->`.

Repo root: {absolute path}
Branch: {branch}
Platform: {os}  (Windows: use bash syntax, forward slashes)

Status: Stages 1..{N-1} committed:
- `{sha1}` - {subject1}
- `{sha2}` - {subject2}
Confirm with: `git log --oneline -{N-1}` (where the integer is the count of
prior stages, not the current stage number).

Line-number hints in the plan may be stale after prior stages; grep for symbols.

Your scope: Stage {N} only - {title}. Items: {IDs}.

Critical rules (from CLAUDE.md):
- Build check: {cmd}
- Other gates: {list}
- Invariants: {logging, i18n, vendor-neutrality, English-only, etc.}

Working tree: per `## Execution policy` working-tree policy = `{policy}`.
- clean-required / stash-authorized: tree is clean at stage start; stage only
  files YOU modify, by explicit path; never `git add -A`.
- integrate-existing: pre-existing dirty files listed in Stage 0 report MAY be
  part of your declared file list; if so, stage them; otherwise leave untouched.

Files to modify:
1. `{path}` - {intent}
...

Order of operations:
1. ...
{last}. Gates pass -> write the post-stage report (copy `docs/plans/_report-template.md`
       as a starting point; leave the `Commit:` slot as `_filled by parent_`)
       -> stage code files AND the report file together by explicit path
       -> commit with HEREDOC including the
       `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer.
       (One commit per stage; report is part of that commit.)

Conventions: see `## Hand-off conventions` in this plan — it covers
Authorization, Scope discipline, Failure protocol, and Return-to-parent
format. They apply to this stage.

Begin now.
```
