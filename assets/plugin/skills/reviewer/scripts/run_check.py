#!/usr/bin/env python3
"""run_check.py — timeout-bounded command execution with last-200-lines capture.

Usage: python run_check.py --timeout <seconds> -- <cmd> <args...>

Exits with the wrapped command's return code on completion. On timeout, exits 124
and prints `timeout (<elapsed>s)`.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import time


def main() -> int:
    ap = argparse.ArgumentParser(description="Run a command with a timeout and capture tail output.")
    ap.add_argument("--timeout", type=float, required=True, help="Timeout in seconds")
    ap.add_argument("cmd", nargs=argparse.REMAINDER, help="Command after `--`")
    args = ap.parse_args()

    cmd = args.cmd
    if cmd and cmd[0] == "--":
        cmd = cmd[1:]
    if not cmd:
        print("error: no command given", file=sys.stderr)
        return 2

    start = time.monotonic()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=args.timeout,
            check=False,
        )
    except subprocess.TimeoutExpired:
        elapsed = time.monotonic() - start
        print(f"timeout ({elapsed:.0f}s)", file=sys.stderr)
        return 124

    combined = (proc.stdout or "") + (proc.stderr or "")
    tail = "\n".join(combined.splitlines()[-200:])
    if tail:
        sys.stdout.write(tail)
        if not tail.endswith("\n"):
            sys.stdout.write("\n")
    return proc.returncode


if __name__ == "__main__":
    sys.exit(main())
