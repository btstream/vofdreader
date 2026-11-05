#!/usr/bin/env python3
"""Fixed-redraw shadow A/B; writes local logs, never uploads document data."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--file", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--binary", default="target/release/ofd-app")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--frames", type=int, default=180)
    args = parser.parse_args()
    if not 1 <= args.repeats <= 20 or not 1 <= args.frames <= 10000:
        parser.error("invalid repeats/frames")
    spec = importlib.util.spec_from_file_location("profile_gpu", Path(__file__).with_name("profile-gpu.py"))
    sampler = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(sampler)
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    reports = []
    for trial in range(args.repeats):
        # Reverse order on alternate trials to reduce order bias.
        for disabled in ([False, True] if trial % 2 == 0 else [True, False]):
            name = f"{trial}-{'no-shadow' if disabled else 'default'}"
            env = os.environ.copy()
            env.pop("OFD_GPU_DISABLE_DOTS", None)
            env.pop("OFD_GPU_DISABLE_PAGE_SHADOWS", None)
            env.update(OFD_GPU_DIAGNOSTICS="1", OFD_GPU_BENCH_FRAMES=str(args.frames))
            if disabled:
                env["OFD_GPU_DISABLE_PAGE_SHADOWS"] = "1"
            log_path = output / f"{name}.log"
            samples = []
            with log_path.open("w") as log:
                app = subprocess.Popen([args.binary, args.file], env=env, stdout=log, stderr=log)
                try:
                    start = last = time.monotonic()
                    previous = sampler.snapshot(app.pid)
                    while app.poll() is None and time.monotonic() - start < 90:
                        time.sleep(0.2)
                        now = time.monotonic()
                        current = sampler.snapshot(app.pid)
                        row = sampler.summarize(previous, current, now - last)
                        row.update(unix_s=time.time(), interval_s=now-last)
                        samples.append(row)
                        previous, last = current, now
                        if any(marker in log_path.read_text() for marker in ["[gpu-bench] totals", "[gpu-bench] invalid"]):
                            break
                finally:
                    app.terminate()
                    try:
                        app.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        app.kill()
                        app.wait()
            text = log_path.read_text()
            begin = re.search(r"begin unix_s=([\d.]+)", text)
            end = re.search(r"end unix_s=([\d.]+)", text)
            totals = re.search(r"totals frames=(\d+) page_draws=(\d+) first_uses=(\d+)", text)
            accepted = bool(begin and end and totals and "[gpu-bench] invalid" not in text)
            valid = [] if not accepted else [s for s in samples if s["unix_s"]-s["interval_s"] >= float(begin[1]) and s["unix_s"] <= float(end[1])]
            engines = sorted({e for s in valid for e in s["engine_busy_pct"]})
            report = {"trial": trial, "no_shadow": disabled, "valid": accepted, "samples": len(valid),
                      "engine_mean_pct": {e: statistics.mean(s["engine_busy_pct"].get(e, 0) for s in valid) for e in engines},
                      "totals": list(map(int, totals.groups())) if totals else None}
            reports.append(report)
            (output / f"{name}.json").write_text(json.dumps(samples, indent=2))
            print(json.dumps(report), flush=True)
    (output / "summary.json").write_text(json.dumps(reports, indent=2))


if __name__ == "__main__":
    main()
