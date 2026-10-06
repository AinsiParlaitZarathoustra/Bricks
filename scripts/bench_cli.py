#!/usr/bin/env python3
"""CLI startup, binary size and peak memory: `bricks` vs other agent CLIs.

Only `--version` is run: no model is called, nothing is billed. Each command
runs `--iterations` times after 3 warm-up runs; the table gives the mean,
min and max wall time, the size of the file actually executed (symlinks
resolved; for a script, its interpreter is not counted) and the peak
resident memory of one run (`/usr/bin/time -l` on macOS, `-v` on Linux).

    python3 scripts/bench_cli.py --bricks path/to/bricks --iterations 50
"""

import argparse
import os
import platform
import shutil
import statistics
import subprocess
import sys
import time


def resolve(cmd):
    path = shutil.which(cmd) if os.sep not in cmd else cmd
    return os.path.realpath(path) if path and os.path.exists(path) else None


def wall_times(path, iterations):
    for _ in range(3):
        subprocess.run([path, "--version"], capture_output=True)
    times = []
    for _ in range(iterations):
        t = time.perf_counter()
        r = subprocess.run([path, "--version"], capture_output=True)
        times.append((time.perf_counter() - t) * 1000)
        if r.returncode != 0:
            raise SystemExit(f"{path} --version failed: {r.stderr.decode()[:200]}")
    return times


def peak_rss_mb(path):
    if sys.platform == "darwin":
        r = subprocess.run(["/usr/bin/time", "-l", path, "--version"], capture_output=True, text=True)
        for line in r.stderr.splitlines():
            if "maximum resident set size" in line:
                return int(line.split()[0]) / (1024 * 1024)
    else:
        r = subprocess.run(["/usr/bin/time", "-v", path, "--version"], capture_output=True, text=True)
        for line in r.stderr.splitlines():
            if "Maximum resident set size" in line:
                return int(line.split(":")[1]) / 1024
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bricks", default="bricks")
    ap.add_argument("--iterations", type=int, default=50)
    ap.add_argument("--others", default="claude,codex")
    a = ap.parse_args()
    print(f"platform: {platform.system()} {platform.machine()} · iterations: {a.iterations}")
    print("| CLI | version | startup mean | min | max | executed file | peak RSS |")
    print("|---|---|---|---|---|---|---|")
    for name in [a.bricks] + [o for o in a.others.split(",") if o]:
        path = resolve(name)
        if not path:
            print(f"| {name} | not installed | | | | | |")
            continue
        version = subprocess.run([path, "--version"], capture_output=True, text=True).stdout.strip().splitlines()
        times = wall_times(path, a.iterations)
        size = os.path.getsize(path) / (1024 * 1024)
        rss = peak_rss_mb(path)
        label = os.path.basename(name)
        print(
            f"| {label} | {version[0] if version else '?'} | {statistics.mean(times):.1f} ms | "
            f"{min(times):.1f} ms | {max(times):.1f} ms | {size:.1f} MB | "
            f"{f'{rss:.1f} MB' if rss is not None else '?'} |"
        )


if __name__ == "__main__":
    main()
