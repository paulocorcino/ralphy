#!/usr/bin/env python3
"""run_check.py — timeout-bounded command execution with last-200-lines capture.

Usage: python run_check.py --timeout <seconds> -- <cmd> <args...>

Exits with the wrapped command's return code on completion. On timeout, exits 124
and prints `timeout (<elapsed>s)`.

Output goes to a temp file, not pipes: grandchildren that outlive a timed-out
direct child (e.g. cargo test's test binaries on Windows) inherit the capture
handles, and draining a pipe they still hold open blocks forever. A file has no
reader to block on. On timeout the whole process tree is killed before the tail
is read.
"""

from __future__ import annotations

import argparse
import os
import signal
import subprocess
import sys
import tempfile
import time


def _kill_tree(proc: subprocess.Popen) -> None:
    if os.name == "nt":
        subprocess.run(
            ["taskkill", "/PID", str(proc.pid), "/T", "/F"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    else:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass


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

    popen_kwargs: dict = {}
    if os.name == "nt":
        popen_kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        popen_kwargs["start_new_session"] = True

    start = time.monotonic()
    with tempfile.TemporaryFile(mode="w+", encoding="utf-8", errors="replace") as out:
        proc = subprocess.Popen(cmd, stdout=out, stderr=out, **popen_kwargs)
        timed_out = False
        try:
            proc.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            _kill_tree(proc)

        out.seek(0)
        tail = "\n".join(out.read().splitlines()[-200:])
        if tail:
            sys.stdout.write(tail)
            if not tail.endswith("\n"):
                sys.stdout.write("\n")

    if timed_out:
        elapsed = time.monotonic() - start
        print(f"timeout ({elapsed:.0f}s)", file=sys.stderr)
        return 124
    return proc.returncode


if __name__ == "__main__":
    sys.exit(main())
