# Writing tests

A test exists to fail when the behavior it names breaks, and only then. This
file says what that requires in Ralphy. Where tests live and the CI gate are in
[AGENTS.md](../AGENTS.md). Measured platform traps are in
[CONTEXT.md → *Testing conventions*](../CONTEXT.md#testing-conventions).

## Words

- **Mutation**: one small edit to production code that breaks the behavior,
  for example deleting a call, `&&` → `||`, `>=` → `>`.
- **Red**: the test fails. **Seen red**: you watched it fail under a mutation.
- **Control**: an assertion that proves the setup could have produced the bad
  result, so a negative assertion cannot pass for the wrong reason.
- **Pin**: a test that reads source text and checks what is or is not there.

## Definition of done

A test is done when every item holds:

1. **Seen red, alone.** You applied a named mutation to production code, saw the
   test fail, and reverted. Run the crate's suite under the same mutation: if
   another test already goes red, the new test adds nothing. Extend that test
   instead, or pick a mutation only yours catches. The commit body names it:
   `red: <file> <mutation> fails <test>`. For a bug fix, the unfixed code is the
   mutation.
2. **It drives production.** The test calls the real entry point and asserts
   what comes out. Every step the test name promises runs in production code.
3. **Negatives have a control.** Every "X did not happen" also shows that X
   could happen.
4. **Values are exact.** It asserts the full value, and the boundary value when
   the code has a boundary.
5. **It tests Ralphy.** The code under test is Ralphy's.
6. **One path, one test.** Cases that differ only in input are rows of one
   table-driven test.
7. **It checks behavior.** A refactor that keeps the behavior keeps it green.

## What earns a test

Test a behavior a caller depends on: a decision, a parse, a boundary, a guard,
a state change, a bug that happened. Glue code that only passes values along is
covered by the test of the behavior it serves. Each behavior has one owner
test; a change to that behavior edits the owner, and a new test appears only
for a new behavior.

## The rules behind each item

### Drive production

The test builds the input. Production does the work. The test stays out of the
work in these ways:

- It leaves the guard, the env strip, the wiring, and the orchestration to
  production and asserts their effect.
- A test helper builds fixtures and fake child output, and never the result
  under assertion.
- A closure or locator the test passes in proves only the code that uses it.
  Name the test for that, and cover the real wiring separately.

When production offers no way in, add a small seam (an injected closure, a
`#[cfg(test)]` hook) or test through the binary. That is a production change:
make it deliberately, not inside a test cleanup.

Quick check: delete the production line the test name promises. The test must
go red.

### Controls for negative assertions

- **Confinement:** put a real file at the escaping target, with known content.
  "Not found" then means refused.
- **"Nothing arrived":** after the silent window, send a valid request on the
  same connection and assert its answer. That separates *silent* from *closed*
  or *errored*.
- **"Not overwritten":** seed content different from what the code writes, and
  compare bytes.
- **"Never read":** the thing that must not be read exists.

### Security guards

A test that guards confinement, auth, tokens, cookies, bind address, CSP,
clipboard, credentials, or autonomy argv:

1. attempts the violation for real;
2. asserts the refusal;
3. includes a legitimate case in the same test and asserts it succeeds;
4. is seen red with the guard itself removed.

### Exact values

Assert with `assert_eq!` on the whole value: `Some(None)`, `"*\n"`, the full
string. Keep `is_some()`, `is_ok()`, `contains`, and `typeof` for values whose
rest varies for a reason you can state. A result that depends on what the host
has installed is injected, or it becomes an `#[ignore]` probe.

### Behavior over form

Assert what a caller observes: the returned value, the state after the call,
the request sent, the file written. Argument order, call counts, exact
attribute lists, statement text, and user-facing wording change on harmless
edits; the `ui-copy` lint owns wording.

Library behavior (std, serde, clap, `fs`) and constants belong to their owners;
test the Ralphy function that uses them.

## Choosing the kind of test

Use the first kind that can see the behavior:

1. A Rust unit or integration test that calls the code or runs the binary.
2. A `node --test` test in `crates/ralphy-daemon/ui-tests` for workbench JS.
   Stub only the browser and network edges; the function under test is real.
3. A pin, for what the first two cannot see: a call site is wired, a forbidden
   thing is absent from production code, a CSS rule exists.

A pin follows these rules:

- **Production text only.** Cut the file at the `#[cfg(test)]` line that is
  followed by `mod `. An item-level `#[cfg(test)] fn` can come before real code.
  Use the crate's existing cut helper.
- **The call, inside the caller.** Slice the body of the function that must make
  the call, then match the call expression. A match on a definition stays green
  when every call is gone.
- **The consequence.** For confirm-then-act, pin `if (ok) <action>`, not the
  dialog title.
- **One owner.** When a behavior test covers the code, the pin goes.
- **A scanner proves it scans.** A lint or guard over many files has a known-bad
  fixture it must catch.

## Cost

The suite is bound by process creation on Windows (see
[BUILDING.md](./BUILDING.md)).

- Add an integration test to an existing file in the crate's `tests/`. A new
  binary needs its own process state (env vars, a `static Once`, a global), and
  its first comment says so.
- Wait on a condition with a bounded timeout. Inject short timeouts.
- Build fixture repos with the fewest `git` calls that reach the needed state.
