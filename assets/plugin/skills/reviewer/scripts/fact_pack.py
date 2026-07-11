#!/usr/bin/env python3
"""fact_pack.py — emit a project-agnostic ground-truth file inventory for the reviewer skill.

Output JSON to stdout. The reviewer's `## Coverage` section is reconciled
against `material_files` and `excluded_files` by audit.py. This script does
not classify, score, rank, or recommend; it only enumerates what is on disk
between target and base, with deterministic exclusions.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import subprocess
import sys
from pathlib import Path, PurePosixPath
from typing import Iterable

LOCKFILES = {
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "Cargo.lock",
    "poetry.lock",
    "Pipfile.lock",
    "composer.lock",
    "go.sum",
}

BUILD_DIR_PARTS = {
    "dist",
    "build",
    "out",
    ".next",
    "coverage",
    "node_modules",
    "vendor",
    "__pycache__",
    "target",
}

CANDIDATE_SPEC_DIRS = [
    "docs/adr",
    "docs/adrs",
    "docs/architecture/decisions",
    "docs/decisions",
    "adr",
    "adrs",
    "decisions",
    "architecture/decisions",
    "docs/rfc",
    "docs/rfcs",
    "rfcs",
    "docs/specs",
    "specs",
    "docs/prd",
    "prd",
    "docs/design",
    "design-docs",
]

CANDIDATE_MANIFESTS = [
    "package.json",
    "deno.json",
    "deno.jsonc",
    "go.mod",
    "Cargo.toml",
    "pyproject.toml",
    "requirements.txt",
    "Pipfile",
    "setup.py",
    "Makefile",
    "Dockerfile",
    "Dockerfile.dev",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
    "manifest.yaml",
    ".gitlab-ci.yml",
    "azure-pipelines.yml",
    "Jenkinsfile",
]

# "Code generated" covers the Go convention (`// Code generated ... DO NOT
# EDIT.`) and its `#` variant; "@generated" covers the Facebook/Buck-style
# marker regardless of comment leader.
GENERATED_MARKERS = ("@generated", "Code generated")
GENERATED_SCAN_LINES = 5


def run_git(repo: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=str(repo),
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return ""
    return result.stdout.rstrip("\n")


def resolve_base(repo: Path, requested: str | None) -> str:
    """Return a verified base ref, or "" when none resolves.

    An explicitly requested base is verified too — an unresolvable base
    would otherwise yield an empty diff and a silent `material: 0` /
    `audit: pass`, which is a false green.
    """
    candidates = (
        [requested] if requested else ["origin/main", "origin/master", "main", "master"]
    )
    for candidate in candidates:
        sha = run_git(repo, "rev-parse", "--verify", "--quiet", candidate)
        if sha:
            return candidate
    return ""


def list_changed(repo: Path, target: str, base: str) -> list[str]:
    if not base:
        return []
    if target == "working-tree":
        diff = run_git(repo, "diff", "--name-only", base, "--")
        untracked = run_git(repo, "ls-files", "--others", "--exclude-standard")
        files = set(filter(None, diff.splitlines())) | set(filter(None, untracked.splitlines()))
        return sorted(files)
    diff = run_git(repo, "diff", "--name-only", f"{base}...{target}", "--")
    return sorted(filter(None, diff.splitlines()))


def is_in_build_dir(rel: str) -> bool:
    parts = PurePosixPath(rel).parts
    return any(part in BUILD_DIR_PARTS for part in parts)


def is_lockfile(rel: str) -> bool:
    return PurePosixPath(rel).name in LOCKFILES


def is_binary(path: Path) -> bool:
    try:
        with path.open("rb") as fh:
            chunk = fh.read(4096)
    except OSError:
        return False
    if not chunk:
        return False
    try:
        chunk.decode("utf-8")
    except UnicodeDecodeError:
        return True
    return False


def is_generated(path: Path) -> bool:
    """Check the first GENERATED_SCAN_LINES non-empty lines for a marker.

    Markers frequently sit below a shebang or license header, so inspecting
    only the first non-empty line misses them.
    """
    try:
        with path.open("r", encoding="utf-8", errors="ignore") as fh:
            seen = 0
            for raw in fh:
                line = raw.strip()
                if not line:
                    continue
                if any(marker in line for marker in GENERATED_MARKERS):
                    return True
                seen += 1
                if seen >= GENERATED_SCAN_LINES:
                    return False
    except OSError:
        return False
    return False


def classify(repo: Path, rel: str) -> tuple[str, str | None]:
    """Return ("material", None) or ("excluded", reason)."""
    if is_lockfile(rel):
        return "excluded", "lockfile"
    if is_in_build_dir(rel):
        return "excluded", "build artifact"
    abs_path = repo / rel
    if not abs_path.exists():
        # Deleted file: treat as material; reviewer must place it explicitly.
        return "material", None
    if is_binary(abs_path):
        return "excluded", "binary"
    if is_generated(abs_path):
        return "excluded", "generated"
    return "material", None


def existing_dirs(repo: Path, candidates: Iterable[str]) -> list[str]:
    out = []
    for c in candidates:
        if (repo / c).is_dir():
            out.append(c)
    return out


def existing_files(repo: Path, candidates: Iterable[str]) -> list[str]:
    out = []
    for c in candidates:
        if (repo / c).is_file():
            out.append(c)
    return out


def find_package_roots(repo: Path) -> list[str]:
    """Locate directories containing a package manifest.

    Uses os.walk with in-place pruning so excluded dirs (node_modules,
    target, .git, ...) are never descended into — rglob-then-filter walks
    them anyway, which is minutes of stat calls on a large monorepo.
    """
    markers = {"package.json", "go.mod", "Cargo.toml", "pyproject.toml"}
    roots: set[str] = set()
    for dirpath, dirnames, filenames in os.walk(repo):
        dirnames[:] = [
            d for d in dirnames if d not in BUILD_DIR_PARTS and d != ".git"
        ]
        if markers & set(filenames):
            roots.add(Path(dirpath).relative_to(repo).as_posix())
    return sorted(roots) if roots else ["."]


def main() -> int:
    ap = argparse.ArgumentParser(description="Emit ground-truth review inventory as JSON.")
    ap.add_argument("--repo", default=".", help="Path to repo root (default: current dir)")
    ap.add_argument("--target", default="working-tree", help="Target ref or 'working-tree'")
    ap.add_argument("--base", default=None, help="Base ref (default: origin/main → main)")
    ap.add_argument(
        "--out",
        default=None,
        help=(
            "Write JSON to this path (UTF-8) instead of stdout. Prefer this "
            "over shell redirection on Windows, where PowerShell's `>` "
            "re-encodes stdout (UTF-16 on 5.1) and breaks the UTF-8 read "
            "in audit.py."
        ),
    )
    args = ap.parse_args()

    repo = Path(args.repo).resolve()
    if not (repo / ".git").exists():
        print(json.dumps({"error": f"not a git repo: {repo}"}), file=sys.stderr)
        return 1

    base = resolve_base(repo, args.base)
    if not base:
        print(
            json.dumps(
                {
                    "error": (
                        f"cannot resolve base ref "
                        f"({args.base or 'origin/main, origin/master, main, master'}): "
                        "ref does not exist in this repo; pass a valid --base"
                    )
                }
            ),
            file=sys.stderr,
        )
        return 1
    head = run_git(repo, "rev-parse", "HEAD")
    branch = run_git(repo, "rev-parse", "--abbrev-ref", "HEAD")
    changed = list_changed(repo, args.target, base)

    material: list[str] = []
    excluded: list[dict] = []
    for rel in changed:
        kind, reason = classify(repo, rel)
        if kind == "material":
            material.append(rel)
        else:
            excluded.append({"path": rel, "reason": reason})

    payload = {
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "repo": str(repo),
        "target": args.target,
        "base": base,
        "head": head,
        "branch": branch,
        "material_files": material,
        "excluded_files": excluded,
        "spec_directories": existing_dirs(repo, CANDIDATE_SPEC_DIRS),
        "manifests": existing_files(repo, CANDIDATE_MANIFESTS),
        "package_roots": find_package_roots(repo),
    }
    text = json.dumps(payload, indent=2) + "\n"
    if args.out:
        Path(args.out).write_text(text, encoding="utf-8")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
