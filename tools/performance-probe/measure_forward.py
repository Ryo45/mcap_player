#!/usr/bin/env python3
"""Measure forward pruning on SHA256-verified baseline datasets, at 100/1000 ms ticks."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--data-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("use a new output directory")
    baseline = json.loads((args.baseline / "environment.json").read_text())
    datasets = [d for d in baseline["datasets"] if d["case"]["encoding"] == "jpeg" and d["case"]["seconds"] == 20]
    if len(datasets) != 18:
        parser.error("expected the 18 JPEG datasets from the 20-second baseline matrix")
    for d in datasets:
        path = args.data_dir.resolve() / (d["name"] + ".mcap")
        with path.open("rb") as stream:
            assert hashlib.file_digest(stream, "sha256").hexdigest() == d["sha256"], path
    binary = ROOT / "target/release/performance-probe"
    args.output.mkdir(parents=True)
    provenance = {"baseline_environment_sha256":hashlib.sha256((args.baseline / "environment.json").read_bytes()).hexdigest(),
                  "binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),
                  "git_commit":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
                  "git_status":subprocess.check_output(["git","status","--short"],cwd=ROOT,text=True),
                  "datasets":datasets,"cache":"uncontrolled/cache-allowed; SHA256-read first", "repeats":3,
                  "cpu_info":Path("/proc/cpuinfo").read_text().split("model name",1)[-1].splitlines()[0],
                  "sources_sha256":{str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest()
                      for p in [Path(__file__).resolve(), ROOT / "tools/performance-probe/src/main.rs",
                                ROOT / "crates/viewer-mcap/src/recording/source.rs",
                                ROOT / "crates/viewer-mcap/src/recording/source/range.rs"]}}
    (args.output / "environment.json").write_text(json.dumps(provenance,indent=2)+"\n")
    rows = []
    with (args.output / "runs.jsonl").open("w") as out:
        for repetition in range(3):
            jobs = [(d,s,t) for d in datasets for s in ("scan","all") for t in (100,1000)]
            random.Random(0x5eed+repetition).shuffle(jobs)
            for d, selection, tick in jobs:
                print(repetition,d["name"],selection,tick,flush=True)
                raw = subprocess.check_output([str(binary),"run",str(args.data_dir.resolve() / (d["name"]+".mcap")),selection,"forward","0",str(tick)],text=True,cwd=ROOT)
                row = json.loads(raw)
                row.update(case=d["name"],repetition=repetition)
                rows.append(row)
                out.write(json.dumps(row,separators=(",",":"))+"\n")
                out.flush()
    lines = ["# Forward pruning measurements", "", "Three fresh processes per condition; times are medians. Cache state uncontrolled.", "",
             "| Case | Selection | Tick ms | Read MiB | Expanded MiB | Work ms | CDR / JPEG decodes |",
             "|---|---|---:|---:|---:|---:|---:|"]
    for d in datasets:
        for selection in ("scan","all"):
            for tick in (100,1000):
                group = [r for r in rows if r["case"]==d["name"] and r["selection"]==selection and r["tick_ms"]==tick]
                median = lambda fn:statistics.median(fn(r) for r in group)
                lines.append(f"| {d['name']} | {selection} | {tick} | {median(lambda r:r['io']['requested_bytes'])/2**20:.3f} | "
                             f"{median(lambda r:r['io']['expanded_bytes'])/2**20:.3f} | {median(lambda r:r['workload_ms']):.2f} | "
                             f"{median(lambda r:r['feature_decoded']):g} / {median(lambda r:r['jpeg_decodes']):g} |")
    (args.output / "summary.md").write_text("\n".join(lines)+"\n")


if __name__ == "__main__":
    main()
