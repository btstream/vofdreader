#!/usr/bin/env python3
"""Read Linux per-process DRM fdinfo counters. No root or third-party packages.

Usage: python3 scripts/profile-gpu.py --pid PID --duration 30
Outputs JSON lines. Engine percentages are process engine-time / wall-time,
not whole-device utilization. Memory fields keep their driver's original names.
"""
import argparse
import json
from pathlib import Path
import time


def parse_fdinfo(text):
    fields = dict(line.split(":", 1) for line in text.splitlines() if ":" in line)
    fields = {key.strip(): value.strip() for key, value in fields.items()}
    if "drm-client-id" not in fields:
        return None
    client = (fields.get("drm-driver", "unknown"), fields.get("drm-pdev", ""),
              fields["drm-client-id"])
    engines, memory = {}, {}
    for key, value in fields.items():
        parts = value.split()
        if len(parts) != 2:
            continue
        try:
            number = int(parts[0])
        except ValueError:
            continue
        if key.startswith("drm-engine-") and parts[1] == "ns":
            engines[key.removeprefix("drm-engine-")] = number
        if key.startswith(("drm-memory-", "drm-resident-", "drm-total-", "drm-shared-")):
            factor = {"B": 1, "KiB": 1024, "MiB": 1024 ** 2}.get(parts[1])
            if factor:
                memory[key] = number * factor
    return client, engines, memory


def snapshot(pid):
    clients = {}
    for path in Path(f"/proc/{pid}/fdinfo").glob("*"):
        try:
            parsed = parse_fdinfo(path.read_text())
        except (OSError, UnicodeError):
            continue
        if parsed:
            client, engines, memory = parsed
            # Several descriptors can reference the same DRM client. Never sum
            # duplicate fdinfo counters, which would multiply busy/memory data.
            clients[client] = (engines, memory)
    return clients


def summarize(previous, current, seconds):
    busy, memory = {}, {}
    for client, (engines, allocations) in current.items():
        old = previous.get(client)
        if old:
            for engine, ns in engines.items():
                before = old[0].get(engine)
                if before is not None and ns >= before:
                    busy[engine] = busy.get(engine, 0.0) + (ns - before) / (seconds * 1e9) * 100
        for key, value in allocations.items():
            memory[key] = memory.get(key, 0) + value
    return {"engine_busy_pct": {key: round(value, 3) for key, value in busy.items()},
            "memory_bytes": memory, "clients": len(current)}


def self_test():
    text = "drm-driver: i915\ndrm-client-id: 2\ndrm-engine-render: 100 ns\ndrm-resident-system: 8 KiB\n"
    client, engines, memory = parse_fdinfo(text)
    assert engines == {"render": 100}
    assert memory == {"drm-resident-system": 8192}
    # The snapshot dictionary deduplicates matching client IDs.
    previous = dict([(client, (engines, memory))] * 2)
    assert len(previous) == 1
    report = summarize(previous, {client: ({"render": 500000100}, memory)}, 1.0)
    assert report["engine_busy_pct"]["render"] == 50.0
    assert report["memory_bytes"]["drm-resident-system"] == 8192
    assert parse_fdinfo("pos: 0\nflags: 0100000\n") is None
    assert summarize(previous, {client: ({"render": 0}, memory)}, 1.0)["engine_busy_pct"] == {}
    print("profile-gpu self-test: OK")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int)
    parser.add_argument("--duration", type=float, default=30)
    parser.add_argument("--interval", type=float, default=1)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.pid or args.pid < 1 or args.duration <= 0 or args.interval <= 0:
        parser.error("--pid PID and positive duration/interval are required")
    process = Path(f"/proc/{args.pid}")
    if not process.exists():
        parser.error("process not found (this tool requires Linux /proc)")
    start = last = time.monotonic()
    previous = snapshot(args.pid)
    if not previous:
        print(json.dumps({"warning": "DRM counters unavailable yet: driver support or fdinfo permission may be missing"}), flush=True)
    while time.monotonic() - start < args.duration and process.exists():
        time.sleep(min(args.interval, max(0, args.duration - (time.monotonic() - start))))
        now = time.monotonic()
        current = snapshot(args.pid)
        report = summarize(previous, current, now - last)
        report["elapsed_s"] = round(now - start, 3)
        report["unix_s"] = time.time()
        print(json.dumps(report, sort_keys=True), flush=True)
        previous, last = current, now


if __name__ == "__main__":
    main()
