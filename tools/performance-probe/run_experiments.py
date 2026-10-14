#!/usr/bin/env python3
"""Reproducible, serial headless experiments; Python stdlib only, no viewer emulation."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import random
import resource
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]


def cases(seconds, large_seconds=0):
    result = []
    for codec in ("none", "zstd", "lz4"):
        for chunk in (64 * 1024, 1024 * 1024, 8 * 1024 * 1024):
            for layout in ("mixed", "grouped"):
                result.append(dict(codec=codec, chunk=chunk, layout=layout,
                                   seconds=seconds, encoding="jpeg"))
    for layout in ("mixed", "grouped"):
        result.append(dict(codec="zstd", chunk=1024 * 1024, layout=layout,
                           seconds=seconds, encoding="raw"))
    for duration in (seconds * 6, seconds * 30):
        result.append(dict(codec="zstd", chunk=1024 * 1024, layout="mixed",
                           seconds=duration, encoding="jpeg"))
    result.append(dict(codec="zstd", chunk=1024 * 1024, layout="mixed",
                       seconds=seconds, encoding="scan"))
    if large_seconds:
        for duration in (max(1, large_seconds // 10), large_seconds):
            for encoding in ("raw", "scan"):
                result.append(dict(codec="none", chunk=1024 * 1024, layout="mixed",
                                   seconds=duration, encoding=encoding))
    return result


def name(case):
    return "{codec}-{chunk}-{layout}-{seconds}s-{encoding}".format(**case)


def checked(command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def percentile(values, fraction):
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * fraction) - 1)]


def summarize(output):
    rows = [json.loads(line) for line in (output / "runs.jsonl").read_text().splitlines()]
    groups = {}
    for row in rows:
        groups.setdefault((row["case"], row["selection"], row["mode"]), []).append(row)
    lines = ["# Headless measurements", "", "Fresh process per repetition; cache state uncontrolled/cache-allowed.",
             "Times are medians across runs. MiB = 2^20 bytes. RSS is sampled Linux process RSS, not heap size.", "",
             "| Case | Selection / path | n | Read MiB | Expanded MiB | Work ms | RSS final / HWM MiB | CDR / JPEG decodes |",
             "|---|---|---:|---:|---:|---:|---:|---:|"]
    for (case, selection, mode), group in sorted(groups.items()):
        med = lambda fn: statistics.median(fn(r) for r in group)
        lines.append(f"| {case} | {selection} / {mode} | {len(group)} | "
                     f"{med(lambda r:r['io']['requested_bytes']) / 2**20:.3f} | "
                     f"{med(lambda r:r['io']['expanded_bytes']) / 2**20:.3f} | "
                     f"{med(lambda r:r['workload_ms']):.2f} | "
                     f"{med(lambda r:r['final_memory']['rss_kib']) / 1024:.2f} / "
                     f"{med(lambda r:r['final_memory']['hwm_kib']) / 1024:.2f} | "
                     f"{med(lambda r:r['feature_decoded']):g} / {med(lambda r:r['jpeg_decodes']):g} |")
    lines += ["", "## Seek read primitives", "",
              "Percentiles are per-run, then medians across runs; the first cache-fill request is included.", "",
              "| Case | Path | n seeks/run | p50 ms | p95 ms | p99 ms | Expanded MiB/run |",
              "|---|---|---:|---:|---:|---:|---:|"]
    for (case, selection, mode), group in sorted(groups.items()):
        if not group[0]["seek_samples"]:
            continue
        def quantile(q):
            return statistics.median(percentile([s["ms"] for s in r["seek_samples"]], q) for r in group)
        lines.append(f"| {case} | {mode} | {len(group[0]['seek_samples'])} | "
                     f"{quantile(.5):.3f} | {quantile(.95):.3f} | {quantile(.99):.3f} | "
                     f"{statistics.median(r['io']['expanded_bytes'] for r in group)/2**20:.3f} |")
    (output / "summary.md").write_text("\n".join(lines) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--seconds", type=int, default=20)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--seeks", type=int, default=64)
    parser.add_argument("--large-seconds", type=int, default=0,
                        help="Additional uncompressed raw / scan-only duration pair; 600 uses about 3 GiB")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/performance-probe")
    parser.add_argument("--summarize-only", action="store_true")
    args = parser.parse_args()
    if args.summarize_only:
        summarize(args.output)
        return
    if platform.system() != "Linux":
        parser.error("RSS and storage I/O measurement currently require Linux /proc")
    if not (1 <= args.seconds <= 2880 and 1 <= args.repeats <= 20 and 1 <= args.seeks <= 10000):
        parser.error("seconds 1..2880, repeats 1..20, seeks 1..10000 required")
    if not (args.large_seconds == 0 or 10 <= args.large_seconds <= 86400):
        parser.error("large-seconds must be 0 or 10..86400")
    if args.output.exists():
        parser.error("output must be new (no mixing independent experiment runs)")
    args.output.mkdir(parents=True)
    args.data_dir.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve()
    datasets = cases(args.seconds, args.large_seconds)
    manifest = []
    for case in datasets:
        path = args.data_dir.resolve() / (name(case) + ".mcap")
        if path.exists():
            raise RuntimeError(f"Dataset exists: {path}; use a new data directory")
        print("generate", path.name, flush=True)
        checked([str(binary), "generate", str(path), case["codec"], str(case["chunk"]),
                 case["layout"], str(case["seconds"]), case["encoding"]])
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        manifest.append(dict(case=case, name=name(case), path=str(path),
                             bytes=path.stat().st_size, sha256=digest))
    provenance = {"git_commit":checked(["git", "rev-parse", "HEAD"]),
                  "git_status":checked(["git", "status", "--short"]),
                  "source_diff_sha256":hashlib.sha256(subprocess.check_output(["git", "diff", "HEAD"], cwd=ROOT)).hexdigest(),
                  "binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),
                  "probe_sources_sha256":{str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest()
                                          for p in Path(__file__).resolve().parent.rglob("*.rs")},
                  "python":platform.python_version(), "platform":platform.platform(),
                  "cpu_info":Path("/proc/cpuinfo").read_text().split("model name", 1)[-1].splitlines()[0],
                  "cpu_affinity":sorted(os.sched_getaffinity(0)),
                  "meminfo":Path("/proc/meminfo").read_text(), "rustc":checked(["rustc", "-Vv"]),
                  "settings":vars(args) | {"data_dir":str(args.data_dir), "output":str(args.output), "binary":str(binary)},
                  "cache":"uncontrolled/cache-allowed, generated and SHA256-read before measurement; no drop_caches",
                  "datasets":manifest}
    (args.output / "environment.json").write_text(json.dumps(provenance, indent=2) + "\n")
    jobs = []
    for entry in manifest:
        case = entry["case"]
        for mode in ("forward", "indexed"):
            for selection in ("scan", "all"):
                jobs.append((entry, selection, mode))
        if case["layout"] == "mixed" and case["encoding"] == "jpeg" and case["seconds"] == args.seconds:
            for mode in ("seek", "seek-cached", "repeat", "repeat-cached"):
                jobs.append((entry, "scan", mode))
    with (args.output / "runs.jsonl").open("w") as result:
        for repetition in range(args.repeats):
            shuffled = list(jobs)
            random.Random(0x5EED + repetition).shuffle(shuffled)
            for entry, selection, mode in shuffled:
                print(repetition + 1, entry["name"], selection, mode, flush=True)
                usage_before = resource.getrusage(resource.RUSAGE_CHILDREN)
                started = time.perf_counter()
                row = json.loads(checked([str(binary), "run", entry["path"], selection, mode, str(args.seeks)]))
                usage_after = resource.getrusage(resource.RUSAGE_CHILDREN)
                row.update(case=entry["name"], repetition=repetition,
                           external_wall_ms=(time.perf_counter()-started)*1000,
                           cpu_user_ms=(usage_after.ru_utime-usage_before.ru_utime)*1000,
                           cpu_system_ms=(usage_after.ru_stime-usage_before.ru_stime)*1000)
                result.write(json.dumps(row, separators=(",", ":")) + "\n")
                result.flush()
    summarize(args.output)


if __name__ == "__main__":
    main()
