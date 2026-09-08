# Ralphy tells its own users that a new build exists: a fragment per pull request, a leaf crate that reads the releases, one outbound GET

Status: accepted (2026-09-08).

Ralphy has users. They run the `v0.1.0-rcNN` pre-releases, which are cut every
few days while feedback is collected, and the project has no way to tell them a
newer build exists, no readable account of what changed, and no upgrade path
shorter than "find the Releases page, download an archive, unpack it, run
`ralphy install`". The upgrade latency that path imposes is what currently costs
the project its feedback: a fix ships and the person who reported the problem is
still on the build that has it.

Four facts constrain any design here, and each was measured rather than assumed.

**`GET /repos/paulocorcino/ralphy/releases/latest` returns 404.** Every tag
carries a hyphen, so the release workflow publishes it `--prerelease`, and the
GitHub API excludes pre-releases from `latest`. A watch that reads the obvious
endpoint is blind for as long as the project stays in release candidates, which
is a deliberate decision and not a transient state.

**`rc19` sorts below `rc9`.** `rc19` is a single alphanumeric pre-release
identifier, so semver compares it ASCII-wise; a correct comparator fed the
current tags would report rc9 as newer than rc19.

**The build embeds a string that is not a version.** `git describe --tags
--always --dirty` yields `v0.1.0-rc19-18-gb2cc208` on a working tree, and
without `--match 'v*'` a `ralphy/pre-run-*` tag can shadow the version tag
entirely. Anything that compares `RALPHY_VERSION` to a release tag by string
equality nags on every development build.

**The release notes are `--generate-notes`.** That folds the commit subjects,
and this repo's subjects are declarative sentences about the change — "a page
that comes back from a suspend reconnects its sockets" — not names of a
capability. The voice is deliberate and stays; it is simply not a changelog. No
commit-derived generator (git-cliff, release-please, semantic-release) repairs
that, because the input is wrong rather than the format.

This ADR decides how a change becomes an announcement, how a running Ralphy
learns of a newer one, and how the operator takes it. It **amends the
"only HTTP client" invariant** stated in `peer/client.rs` (§4) and adds a
release-communication obligation to every pull request (§1). ADR-0032 §10
(the daemon never imports the core) and ADR-0002 (the core/adapter seam) are
not reopened; §4 exists precisely to respect them.

## Decision

### 1. The unit of release communication is a fragment written in the pull request

> A pull request that changes what a user can see or do carries
> **`changelog.d/<n>.md`**: a `kind:` front-matter field drawn from a closed
> set — `feature`, `fix`, `breaking`, `security`, `internal` — and one or two
> sentences naming the capability in the user's terms.

The fragment is written when the change is written, by whoever wrote it, and it
is written in English like every other artifact (CLAUDE.md). It is the only
human-authored release text; everything downstream is derived from it.

**Rejected: generating the notes from commit subjects.** The subject names the
change; the fragment names the capability. "The virtual keyboard no longer
paints over the prompt" is an accurate commit and a useless release note,
because the reader does not know a virtual keyboard was ever in the way.

**Rejected: one shared `CHANGELOG.md` edited per pull request.** Two agents
working in parallel is this repo's normal state, and a single hot file is a
guaranteed conflict on every second branch. One file per pull request has no
conflict surface at all.

**Rejected: curating at tag time.** Reconstructing an account of twenty merges
from the log is work no one sustains past the third release, and it is performed
at the moment the author has forgotten the most.

### 2. The fold is an `xtask`, and `CHANGELOG.md` is machine-owned

`cargo run -p xtask -- changelog` folds the fragments into `CHANGELOG.md`, a
`notes.md` for the release body, and a `changelog.json` for the workbench. It
inherits the whole `refresh-seed` contract: out-of-band, never `build.rs`,
deterministic output so the diff is reviewable, and byte-identical on a no-op
run.

`changelog.d/` is human-owned and `CHANGELOG.md` is machine-owned. That is
ADR-0034 A3's one-owner-per-file rule, applied to a second pair of files rather
than invented here.

The same task carries `bump`: the release ritual today is fifteen hand-edited
`Cargo.toml` version lines plus `Cargo.lock`, with no script behind it, and a
cadence of one release every few days turns that into a recurring drift risk.

### 3. The discipline is a CI gate, and its path filter includes the UI assets

A pull-request-only job fails when a change touches the shipped surface and
carries no fragment. A `no-changelog` label, applied by the human who reviews
and merges, is the escape hatch — which fits the contribution rule already in
CLAUDE.md rather than adding a second authority.

The filter covers `crates/**/src`, **`crates/ralphy-daemon/assets/ui/`** and
**`assets/`** — the prompts, the plugin and the pricing floor, all of which are
embedded in the shipped binary. The second path is not an afterthought: 205 of the last 400 commits are scoped
`workbench` and land in the assets, so a source-only filter would let most
user-visible work through ungated and the gate would certify a discipline that
was not happening.

A machine-opened pull request cannot write a fragment, so the scheduled pricing
refresh carries the `no-changelog` label from its own workflow rather than being
red every week.

**Rejected: a documentation rule alone.** The instruction already exists in
prose; what is missing is the thing that reds a build. A convention nobody can
forget is a convention someone enforced.

### 4. `ralphy-release` is a leaf crate, not a new client inside the daemon

The daemon cannot ask the core: ADR-0032 §10 forbids the edge, so
`ralphy_core::github` — which shells to `gh` — is unreachable from it. And
`peer/client.rs` opens by declaring itself "the daemon's first — and only —
HTTP client", loopback-only, refusing any descriptor that advertises a routable
address.

So the release read lives in a new leaf crate, `ralphy-release`, shaped exactly
like `ralphy-pricing`: `ureq`, `serde`, a disk cache, an injectable URL, no edge
to the core, and therefore admissible under ADR-0032's leaf-crate exception. It
owns version identity, the releases fetch, and the cache. Its two callers are
real and distinct — `ralphy update` in the CLI and the watch in the daemon — so
it is a shared crate rather than an abstraction built for one.

**This amends the "only HTTP client" claim.** Loopback-only remains the rule for
peer traffic, which is what that invariant was written to protect; the release
read is a second, separately named, opt-outable seam, and the amendment is
recorded here so the next reader of `peer/client.rs` finds the exception rather
than a contradiction.

It costs no new dependency tree: `ureq` with rustls and webpki-roots is already
compiled into the daemon through `ralphy-pricing`. `ureq` is blocking, so the
daemon calls it under `spawn_blocking`, as `repos_route` already does for its
`git` spawns.

**Rejected: spawning `gh api` from the daemon.** It would make `gh` a
prerequisite for learning about an update, hand the answer back through a
channel that does not exist, and put the daemon in the business of running the
repo's tooling, which is `ralphy`'s job.

### 5. Version identity: a dotted candidate, a matched describe, and a build that is ahead

Tags become `v0.1.0-rc.N`. The dot makes the candidate number a numeric
pre-release identifier, which is what semver orders correctly; `rc19` as one
alphanumeric token is not. Both `build.rs` scripts pass `--match 'v*'`, so a
`ralphy/pre-run-*` tag can never be read as the version.

**The comparator normalizes both spellings, and it has to.** Changing the tag
format is not sufficient on its own — it is, on its own, worse. Semver compares
pre-release identifiers left to right, so `0.1.0-rc.20` is `rc` then `20` while
`0.1.0-rc19` is the single token `rc19`, and `"rc" < "rc19"`: the first dotted
tag would sort *below* every candidate it replaces, and every operator on rc19
would be told they were current forever. So the comparator splits a trailing
digit run out of each identifier before comparing — `rc19` and `rc.19` become
the same two identifiers — and both spellings then order by the number a human
reads. The alternative was to skip the transition by bumping the patch instead
(`v0.1.1-rc.1`), which orders correctly with no normalization, but it renames
the version being worked towards to buy an ordering property, and it leaves the
nineteen published tags still mutually misordered for anyone comparing them.

A build whose describe carries a commits-ahead suffix or `-dirty` is **ahead of**
the newest release, not behind it. It is never told to update.

**Rejected: comparing the embedded string to the latest tag for inequality.** It
nags on every development build and reads a rollback as an upgrade.

The comparator is `semver`, which is new to the workspace. Hand-rolled
pre-release ordering is precisely the code that rots, and fact three above is the
demonstration.

### 6. Staleness is a TTL over a timestamped envelope; there is no ETag

The cache is `ralphy-pricing`'s: a self-timestamped JSON document, a TTL check
with a clock-skew guard, an atomic temp-plus-rename write, and a fetch failure
that leaves the prior cache alone and returns without error. At four requests a
day an `If-None-Match` round-trip buys nothing measurable and adds a second
concept of staleness to a file that already has one.

**No network is not an error.** A failed fetch is silent — no banner, no log
noise the operator must dismiss, no degraded mode. The workbench simply shows
what it last knew.

### 7. Severity comes from the kind, not from the version delta

While the project is in release candidates there is no minor-versus-patch signal
to read, so the `kind` recorded in §1 is the only severity the system has. It
decides all three surfaces:

- fixes only — a quiet dot on the account puck;
- a feature — a badge and the "What's new" panel;
- breaking or security — a banner that persists until dismissed.

It also decides the announcement: `--discussion-category Announcements` is passed
**only** when the release carries a `feature`, `breaking` or `security`
fragment. At a cadence of one release every few days, announcing every one of
them trains the audience to ignore all of them; every release still lands in the
changelog.

The gap the workbench reports is the **whole list from the installed version to
the newest**, not the newest release alone. At this cadence the typical user is
three or four versions behind, and a panel that shows only the last one hides the
reason to upgrade.

### 8. The update replaces the binary, and the daemon is restarted

`ralphy update` resolves the newest release for its channel, maps the host to the
published target name, downloads the archive, **verifies it against the published
`.sha256`**, unpacks it, and replaces the running binary by rename-then-replace —
a running image on Windows cannot be deleted but can be renamed.

It carries a **channel** (`rc` or `stable`), defaulting to `rc` today, so the
first minor release changes a default instead of stranding the users who want the
candidates.

Replacing the file is not enough: the resident daemon keeps executing the image
it started with. So the daemon writes a pid file into its store, and `ralphy
daemon restart` reads it, kills the tree, and spawns the new binary detached —
the two halves of which already exist as `kill_tree_by_pid` and `spawn_detached`.
Without this the update is a file swap the operator cannot observe.

**Rejected: downloading and instructing.** A half-path is friction that cancels
the benefit; the checksum the release already publishes is what makes the full
path defensible.

**Rejected: unattended update.** Nothing here replaces a binary without the
operator asking for it, in a product whose whole posture is that the operator
decides.

### 9. What this does not build

No telemetry, of any kind, in any direction. Stated exactly, because this is the
clause that has to survive contact with the README's "nothing is uploaded
anywhere": the watch is an unauthenticated `GET` whose entire outbound content is
the page size (`?per_page=10`), a static `User-Agent: ralphy` that GitHub refuses
the request without, and an `Accept` header. No body, no credential, no cookie,
and nothing that distinguishes one installation from another — not a version, not
an operating system, not an identifier of any kind. No unattended or background update. No in-app feedback form.
No chat platform — announcements ride GitHub Discussions, which is searchable,
needs no moderation rota, and does not read as abandoned when it is quiet. No
notification channel outside the workbench (no email, no Telegram release card).
No back-written changelog for the nineteen candidates already published: the
record starts where the discipline starts.

## Consequences

- **Every pull request that touches the shipped surface gains one file.** That is
  the cost, and it is paid by the author who has the context, at the moment they
  have it. The gate makes it non-optional; the label makes it escapable by a
  human, never by an agent.
- **The workspace gains one crate and one comparison dependency.**
  `ralphy-release` has two real callers before it exists, and `semver` replaces
  the hand-rolled ordering that fact three shows is already wrong. `ureq` in the
  daemon adds no compiled tree.
- **The daemon makes an outbound request for the first time.** It is named,
  documented, TTL-cached, silent on failure, and switched off by a marker file in
  the daemon store — the shape `daemon-require-login` already established. The
  claim in `peer/client.rs` is now an amended invariant, not an absolute.
- **`/api/release` is a new route, and `/api/about` is untouched.** `about_route`
  is a stateless handler with no arguments; giving it state would rewrite the
  route and its test to carry a field that belongs to a different lifetime.
- **The release job gains a checkout.** It has no source tree today, only the
  downloaded artifacts, so `--notes-file` is impossible until it does. Anything
  placed in `out/` becomes a release asset: `changelog.json` belongs there and the
  notes markdown does not.
- **A build from a source tarball reports no update.** With no `.git`, describe
  falls back to the manifest version and the comparator compares it honestly. A
  dirty or ahead build is silent by §5.
- **The first minor release is a default change, not a migration.** Channel `rc`
  becomes channel `stable` for new installs, and the tag format is already
  numerically ordered by then.
