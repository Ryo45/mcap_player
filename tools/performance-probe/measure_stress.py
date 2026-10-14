#!/usr/bin/env python3
"""4K / 512 MiB experiments, keeping production limits and recording rejections."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import signal
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]


def measured(command, rss_limit_mib, timeout):
    # GNU time preserves per-child max RSS even for short rejected runs.
    with tempfile.TemporaryDirectory(prefix="mcap-stress-time-") as directory:
        report = Path(directory) / "usage.txt"
        process = subprocess.Popen(["/usr/bin/time", "-f", '{"max_rss_kib":%M,"user_s":%U,"system_s":%S}',
                                    "-o", str(report), *command], cwd=ROOT,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, start_new_session=True)
        started = time.monotonic()
        guard = None
        sampled_peak_kib = 0
        while process.poll() is None:
            if time.monotonic() - started > timeout:
                guard = "timeout"
            children = Path(f"/proc/{process.pid}/task/{process.pid}/children")
            try:
                for pid in children.read_text().split():
                    status = Path(f"/proc/{pid}/status").read_text()
                    rss = next((int(line.split()[1]) for line in status.splitlines() if line.startswith("VmRSS:")), 0)
                    sampled_peak_kib = max(sampled_peak_kib,rss)
                    if rss > rss_limit_mib * 1024:
                        guard = "rss_guard"
            except (FileNotFoundError, ProcessLookupError):
                pass
            if guard:
                os.killpg(process.pid, signal.SIGTERM)
                break
            time.sleep(.02)
        stdout, stderr = process.communicate(timeout=10)
        usage = {}
        if report.exists():
            records = [line for line in report.read_text().splitlines() if line.startswith("{")]
            if records:
                usage = json.loads(records[-1])
        usage["max_sampled_rss_kib"] = sampled_peak_kib
        error = stderr.strip()
        status = "ok" if process.returncode == 0 else "unexpected_error"
        if guard:
            status = guard
        elif process.returncode != 0 and ("exceeds" in error and "limit" in error):
            status = "product_limit"
        return dict(status=status, returncode=process.returncode, stderr=error,
                    external_ms=(time.monotonic()-started)*1000, usage=usage,
                    result=json.loads(stdout) if status == "ok" else None)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rss-limit-mib", type=int, default=1536)
    parser.add_argument("--timeout", type=int, default=180)
    args = parser.parse_args()
    if args.data_dir.exists() or args.output.exists():
        parser.error("data and output directories must both be new")
    if not 512 <= args.rss_limit_mib <= 2048 or args.timeout < 30:
        parser.error("rss limit must be 512..2048 MiB, timeout >= 30 seconds")
    # Conservative uncompressed upper bound for this fixed suite, with space for source masters.
    if shutil.disk_usage(args.data_dir.parent).free < 50 * 2**30:
        parser.error("need at least 50 GiB free in the data filesystem")
    args.data_dir.mkdir(parents=True)
    args.output.mkdir(parents=True)
    binary = ROOT / "target/release/performance-probe"
    environment = {"git_commit":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
                   "git_status":subprocess.check_output(["git","status","--short"],cwd=ROOT,text=True),
                   "binary_sha256":digest(binary), "cpu_info":Path("/proc/cpuinfo").read_text().split("model name",1)[-1].splitlines()[0],
                   "meminfo_before":Path("/proc/meminfo").read_text(), "rss_guard_mib":args.rss_limit_mib,
                   "timeout_s":args.timeout, "cache":"uncontrolled/cache-allowed; generated and SHA256-read first",
                   "pixel_format":"rgba (production viewer-renderer::decode_jpeg)","tick_ms":50,
                   "sources_sha256":{str(p.relative_to(ROOT)):digest(p) for p in [Path(__file__).resolve(),ROOT / "tools/performance-probe/src/main.rs",ROOT / "crates/viewer-mcap/src/recording/source.rs",ROOT / "crates/viewer-renderer/src/image.rs"]}}
    (args.output / "environment.json").write_text(json.dumps(environment,indent=2)+"\n")
    datasets = []

    def generate(name, codec, chunk_mib, layout, seconds, encoding, width, height, variants, hz, count):
        path = args.data_dir.resolve() / (name+".mcap")
        print("generate", name, flush=True)
        job = measured([str(binary),"generate",str(path),codec,str(chunk_mib*2**20),layout,str(seconds),encoding,
                        str(width),str(height),str(variants),str(hz),str(count)],args.rss_limit_mib,args.timeout)
        if job["status"] != "ok":
            (args.output / (name+"-generation-failure.json")).write_text(json.dumps(job,indent=2))
            raise RuntimeError(f"generation failed: {name}: {job['status']}: {job['stderr']}")
        entry = dict(name=name,path=str(path),codec=codec,chunk_mib=chunk_mib,layout=layout,seconds=seconds,
                     encoding=encoding,width=width,height=height,variants=variants,hz=hz,cameras=count,
                     sha256=digest(path),generation=job)
        datasets.append(entry)
        return entry

    def rechunk(master, codec, size):
        name = f"{master['width']}x{master['height']}-{master['cameras']}cam-{master['hz']}hz-{codec}-{size}MiB-{master['encoding']}"
        path = args.data_dir.resolve() / (name+".mcap")
        print("rechunk",name,flush=True)
        job = measured([str(binary),"rechunk",master["path"],str(path),codec,str(size*2**20)],args.rss_limit_mib,args.timeout)
        if job["status"] != "ok":
            (args.output / (name+"-generation-failure.json")).write_text(json.dumps(job,indent=2))
            raise RuntimeError(f"rechunk failed: {name}: {job['status']}: {job['stderr']}")
        entry = master | dict(name=name,path=str(path),codec=codec,chunk_mib=size,
                              sha256=digest(path),generation=job,source_sha256=master["sha256"])
        datasets.append(entry)
        return entry

    master = generate("3840x2160-8cam-10hz-none-8MiB-jpeg","none",8,"mixed",4,"jpeg",3840,2160,32,10,8)
    for codec in ("none","zstd"):
        for size in (8,64,128,256,512):
            if codec == "none" and size == 8:
                continue
            rechunk(master,codec,size)
    for size in (64,512):
        rechunk(master,"lz4",size)
    alternate = generate("3840x2160-4cam-20hz-none-64MiB-jpeg","none",64,"mixed",4,"jpeg",3840,2160,32,20,4)
    rechunk(alternate,"zstd",64)
    generate("3840x2160-8cam-10hz-zstd-512MiB-grouped-jpeg","zstd",512,"grouped",4,"jpeg",3840,2160,32,10,8)
    raw = generate("3840x2160-8cam-10hz-none-64MiB-raw","none",64,"mixed",1,"raw",3840,2160,2,10,8)
    for size in (128,512):
        rechunk(raw,"none",size)
    generate("320x240-8cam-10hz-zstd-64MiB-jpeg","zstd",64,"mixed",4,"jpeg",320,240,32,10,8)
    (args.output / "datasets.json").write_text(json.dumps(datasets,indent=2)+"\n")
    rows = []
    with (args.output / "runs.jsonl").open("w") as out:
        for repetition in range(3):
            jobs = [(d,s,"forward") for d in datasets for s in ("scan","all")]
            # Query read path shows rejection differences, without paying large-window expansion on every supported case.
            jobs += [(d,"scan","indexed") for d in datasets if d["chunk_mib"] == 512 and d["layout"] == "mixed"]
            random.Random(0x5eed+repetition).shuffle(jobs)
            for dataset, selection, mode in jobs:
                print("run",repetition,dataset["name"],selection,mode,flush=True)
                job = measured([str(binary),"run",dataset["path"],selection,mode,"0","50","rgba"],args.rss_limit_mib,args.timeout)
                row = job | dict(case=dataset["name"],selection=selection,mode=mode,repetition=repetition,
                                 recording_seconds=dataset["seconds"])
                rows.append(row)
                out.write(json.dumps(row,separators=(",",":"))+"\n")
                out.flush()
    lines = ["# 4K and large Chunk measurements", "", "Three fresh processes per condition; supported times/RSS are medians. Product limits remain 256 MiB.", "",
             "| Case | Selection / mode | Status (3 runs) | Actual max Chunk MiB | Read MiB | Work ms / recording ms | JPEG decodes | Peak RSS MiB |",
             "|---|---|---|---:|---:|---:|---:|---:|"]
    for d in datasets:
        for selection,mode in [("scan","forward"),("all","forward"),("scan","indexed")]:
            group = [r for r in rows if r["case"]==d["name"] and r["selection"]==selection and r["mode"]==mode]
            if not group:
                continue
            ok = [r for r in group if r["status"]=="ok"]
            value = lambda fn:statistics.median(fn(r["result"]) for r in ok) if ok else None
            display = lambda v:f"{v:.2f}" if v is not None else "—"
            statuses = ", ".join(r["status"] for r in group)
            peak = statistics.median(r["usage"].get("max_rss_kib",0)/1024 for r in group)
            lines.append(f"| {d['name']} | {selection} / {mode} | {statuses} | {d['generation']['result']['largest_chunk_bytes']/2**20:.2f} | "
                         f"{display(value(lambda r:r['io']['requested_bytes']/2**20))} | {display(value(lambda r:r['workload_ms']))} / {d['seconds']*1000} | "
                         f"{display(value(lambda r:r['jpeg_decodes']))} | {peak:.2f} |")
    (args.output / "summary.md").write_text("\n".join(lines)+"\n")
    if any(r["status"] in ("unexpected_error","rss_guard","timeout") for r in rows):
        raise RuntimeError("Non-product failures occurred; inspect runs.jsonl before drawing conclusions")


if __name__ == "__main__":
    main()
