"""The two checks of the lint transition (docs/lint/lint-transition.md).

  python docs/lint/gate.py shrink [--base REF]   # each debt list only shrinks
  python docs/lint/gate.py added  [--base REF]   # the lines a change adds meet the target

`shrink` compares the debt list of each config with the one at the merge base
of HEAD and REF. `added` runs the drafts in docs/lint/lint-drafts (the target as
it will be when the plan closes) and fails on a site on a line the change adds,
whether its rule is on or still on a debt list. REF is `main` by default; CI
passes the base branch of the pull request.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LINT = ROOT / "docs" / "lint"
SCRIPTS = LINT / "scripts"
DRAFTS = LINT / "lint-drafts"
PLAN = LINT / "lint-transition.md"

# The configs that hold a debt list.
CONFIGS = ["Cargo.toml", ".oxlintrc.json"]

UI = "crates/ralphy-daemon/assets/ui"
UI_TESTS = "crates/ralphy-daemon/ui-tests"
OXLINT = ["npx", "-y", "-p", "oxlint@1.93.0", "-p", "oxlint-tsgolint@7.0.2006", "oxlint"]
TSC = ["npx", "-y", "-p", "typescript@7.0.2", "tsc"]
# Rules `added` does not read yet. `tests_outside_test_module` fires on every
# test of a `tests/` crate until its phase puts one `#![expect]` at each crate
# root; that phase removes it from this list.
ADDED_IGNORE = {"clippy": "clippy::tests_outside_test_module"}
# The tsconfig flags of the target that are not on yet; tsconfig.json has them all.
TSC_FLAGS: list[str] = []


def run(cmd: list[str], **kw: object) -> subprocess.CompletedProcess[str]:
    """Run `cmd` from the repo root; on Windows npx is a .cmd, which needs a shell."""
    return subprocess.run(  # noqa: S603 - fixed commands of this script
        cmd, cwd=ROOT, text=True, encoding="utf-8", shell=os.name == "nt", **kw
    )


def merge_base(ref: str) -> str:
    done = run(["git", "merge-base", "HEAD", ref], capture_output=True, check=True)
    return done.stdout.strip()


def shrink(base: str) -> int:
    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        for config in CONFIGS:
            old = Path(tmp) / Path(config).name
            shown = run(["git", "show", f"{base}:{config}"], capture_output=True)
            old.write_text(shown.stdout if shown.returncode == 0 else "", encoding="utf-8")
            check = [sys.executable, str(SCRIPTS / "debt.py"), "shrink", str(old), config]
            failed |= run([*check, "--plan", str(PLAN)]).returncode
    return failed


def tally_added(fmt: str, output: Path, base: str, ignore: str = "") -> int:
    cmd = [sys.executable, str(SCRIPTS / "tally.py"), "added", fmt, str(output), "--base", base]
    if ignore:
        cmd += ["--ignore", ignore]
    return run(cmd).returncode


def clippy_draft(out: Path) -> int:
    lints = (DRAFTS / "clippy-lints.txt").read_text(encoding="utf-8").split()
    # The draft reports its lints as warnings: a `-D warnings` from the
    # environment (CI sets one) would stop the build at the first site.
    env = {**os.environ, "CLIPPY_CONF_DIR": str(DRAFTS)}
    env.pop("RUSTFLAGS", None)
    cmd = ["cargo", "clippy", "--locked", "--workspace", "--all-targets"]
    with out.open("w", encoding="utf-8") as f:
        done = run([*cmd, "--message-format=json", "--", *lints], stdout=f, env=env)
    return done.returncode


def oxlint_draft(out: Path, type_aware: bool) -> None:
    cmd = [*OXLINT, "-c", str(DRAFTS / "oxlintrc.json"), "--ignore-pattern", "**/vendor/**"]
    paths = [UI] if type_aware else [UI, UI_TESTS]
    flags = ["--type-aware"] if type_aware else []
    with out.open("w", encoding="utf-8") as f:
        # The draft finds the debt sites, so it exits 1; only the JSON counts.
        run([*cmd, *flags, "--format", "json", *paths], stdout=f)


def tsc_draft(out: Path) -> None:
    with out.open("w", encoding="utf-8") as f:
        run([*TSC, "--noEmit", "--pretty", "false", "-p", UI, *TSC_FLAGS], stdout=f)


def added(base: str) -> int:
    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp)
        if clippy_draft(out / "clippy.json") != 0:
            print("the clippy draft did not finish", file=sys.stderr)
            return 1
        failed |= tally_added("clippy", out / "clippy.json", base, ADDED_IGNORE["clippy"])
        oxlint_draft(out / "oxlint.json", type_aware=False)
        failed |= tally_added("oxlint", out / "oxlint.json", base)
        oxlint_draft(out / "oxlint-typed.json", type_aware=True)
        failed |= tally_added("oxlint", out / "oxlint-typed.json", base)
        tsc_draft(out / "tsc.txt")
        failed |= tally_added("tsc", out / "tsc.txt", base)
    return failed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("check", choices=["shrink", "added"])
    parser.add_argument("--base", default="main", help="the branch the change goes into")
    args = parser.parse_args()
    base = merge_base(args.base)
    return shrink(base) if args.check == "shrink" else added(base)


if __name__ == "__main__":
    sys.exit(main())
