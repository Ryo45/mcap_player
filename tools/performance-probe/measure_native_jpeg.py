#!/usr/bin/env python3
"""Compare the old RGBA path, TurboJPEG full, IDCT half, and focus/thumbnail policy."""
import argparse
import json
from pathlib import Path
import random
import statistics
import subprocess
from measure_stress import measured, digest, ROOT


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline",type=Path,required=True)
    parser.add_argument("--data-dir",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args()
    if args.output.exists(): parser.error("use a new output directory")
    names=["3840x2160-8cam-10hz-none-8MiB-jpeg","3840x2160-8cam-10hz-zstd-64MiB-jpeg",
           "3840x2160-8cam-10hz-zstd-512MiB-grouped-jpeg","3840x2160-4cam-20hz-zstd-64MiB-jpeg"]
    datasets=[d for d in json.loads((args.baseline/"datasets.json").read_text()) if d["name"] in names]
    if len(datasets)!=4: parser.error("expected four stress baseline datasets")
    for d in datasets:
        path=args.data_dir.resolve()/(d["name"]+".mcap")
        if digest(path)!=d["sha256"]: raise ValueError(f"dataset mismatch: {path}")
    binary=ROOT/"target/release/performance-probe"
    args.output.mkdir(parents=True)
    environment={"baseline_environment_sha256":digest(args.baseline/"environment.json"),"binary_sha256":digest(binary),
        "git_commit":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
        "git_status":subprocess.check_output(["git","status","--short"],cwd=ROOT,text=True),
        "sources_sha256":{str(p.relative_to(ROOT)):digest(p) for p in [Path(__file__).resolve(),ROOT/"crates/viewer-renderer/src/native_jpeg.rs",ROOT/"crates/viewer-renderer/src/camera_decode.rs",ROOT/"tools/performance-probe/src/main.rs"]},
        "datasets":datasets,"tick_ms":50,"repeats":3,"threads_by_mode":{"rgba":1,"native-full":1,"native-half":1,"native-layout":1,"native-layout-workers2":2},
        "worker_barrier":"wait at each 50 ms tick; exactly the same admitted JPEG count; production UI does not wait","cache":"uncontrolled/cache-allowed; dataset SHA256-read first"}
    (args.output/"environment.json").write_text(json.dumps(environment,indent=2)+"\n")
    rows=[]
    with (args.output/"runs.jsonl").open("w") as out:
        for repetition in range(3):
            jobs=[(d,mode) for d in datasets for mode in ["rgba","native-full","native-half","native-layout","native-layout-workers2"]]
            random.Random(0x5eed+repetition).shuffle(jobs)
            for d,mode in jobs:
                print(repetition,d["name"],mode,flush=True)
                job=measured([str(binary),"run",str(args.data_dir.resolve()/(d["name"]+".mcap")),"all","forward","0","50",mode],1536,180)
                row=job|dict(case=d["name"],mode=mode,repetition=repetition)
                rows.append(row);out.write(json.dumps(row,separators=(",",":"))+"\n");out.flush()
                if row["status"]!="ok":raise RuntimeError(row)
    lines=["# Native JPEG decode comparison","","Three fresh processes per condition, CPU decode (workers2 uses two threads and a per-tick barrier). No GPU. Times/RSS are medians.","",
        "| Dataset | Decode | Read ms | JPEG ms | Work ms / 4000 ms log | JPEG count | Retained RGBA MiB | Peak RSS MiB |",
        "|---|---|---:|---:|---:|---:|---:|---:|"]
    for d in datasets:
        for mode in ["rgba","native-full","native-half","native-layout","native-layout-workers2"]:
            group=[r for r in rows if r["case"]==d["name"] and r["mode"]==mode]
            m=lambda f:statistics.median(f(r) for r in group)
            lines.append(f"| {d['name']} | {mode} | {m(lambda r:r['result']['read_ms']):.2f} | {m(lambda r:r['result']['jpeg_ms']):.2f} | "
                f"{m(lambda r:r['result']['workload_ms']):.2f} / 4000 | {m(lambda r:r['result']['jpeg_decodes']):g} | "
                f"{m(lambda r:r['result']['retained_pixel_bytes'])/2**20:.3f} | {m(lambda r:r['usage']['max_rss_kib'])/1024:.2f} |")
    (args.output/"summary.md").write_text("\n".join(lines)+"\n")


if __name__=="__main__": main()
