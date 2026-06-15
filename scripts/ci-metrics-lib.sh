#!/usr/bin/env bash
# Shared CI metrics helpers for scripts that append test-captured Hostlet
# performance observations to a JSON array artifact.

ci_metrics_init() {
  local path="$1"
  mkdir -p "$(dirname "${path}")"
  printf '[]\n' >"${path}"
}

ci_metrics_write_object() {
  local path="$1"
  local mode="$2"
  local raw
  raw="$(cat)"
  mkdir -p "$(dirname "${path}")"
  python3 - "${path}" "${mode}" "${raw}" <<'PY'
import json
import os
import sys
import tempfile

path = sys.argv[1]
mode = sys.argv[2]
entry = json.loads(sys.argv[3])
if not isinstance(entry, dict):
    raise SystemExit("metrics entry must be a JSON object")

try:
    with open(path, encoding="utf-8") as handle:
        payload = json.load(handle)
except FileNotFoundError:
    payload = []

if not isinstance(payload, list):
    raise SystemExit(f"metrics file must contain a JSON array: {path}")

if mode == "append":
    pass
elif mode == "upsert-fixture":
    fixture = entry.get("fixture")
    if not isinstance(fixture, str) or not fixture:
        raise SystemExit("metrics entry must include a non-empty fixture")
    payload = [item for item in payload if not (isinstance(item, dict) and item.get("fixture") == fixture)]
else:
    raise SystemExit(f"unknown metrics write mode: {mode}")

payload.append(entry)

directory = os.path.dirname(path) or "."
fd, tmp_path = tempfile.mkstemp(prefix=f"{os.path.basename(path)}.", suffix=".tmp", dir=directory)
try:
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, indent=2, sort_keys=True)
        handle.write("\n")
    os.replace(tmp_path, path)
finally:
    if os.path.exists(tmp_path):
        os.unlink(tmp_path)
PY
}

ci_metrics_append_object() {
  ci_metrics_write_object "$1" append
}

ci_metrics_upsert_object_by_fixture() {
  ci_metrics_write_object "$1" upsert-fixture
}

ci_docker_ready_stats_json() {
  local container="$1"
  local raw
  raw="$(docker stats --no-stream --format '{{json .}}' "${container}" 2>/dev/null || true)"
  python3 - "${raw}" <<'PY'
import json
import re
import sys

raw = sys.argv[1].strip()
if not raw:
    print("{}")
    raise SystemExit(0)

try:
    stats = json.loads(raw)
except json.JSONDecodeError:
    print("{}")
    raise SystemExit(0)

UNITS = {
    "b": 1,
    "kb": 1000,
    "mb": 1000**2,
    "gb": 1000**3,
    "tb": 1000**4,
    "kib": 1024,
    "mib": 1024**2,
    "gib": 1024**3,
    "tib": 1024**4,
}


def percent(value):
    if not isinstance(value, str):
        return None
    try:
        return float(value.strip().rstrip("%"))
    except ValueError:
        return None


def bytes_value(value):
    if not isinstance(value, str):
        return None
    match = re.match(r"^\s*([0-9]+(?:\.[0-9]+)?)\s*([A-Za-z]+)\s*$", value)
    if not match:
        return None
    multiplier = UNITS.get(match.group(2).lower())
    if multiplier is None:
        return None
    return int(float(match.group(1)) * multiplier)


def split_pair(value):
    if not isinstance(value, str) or "/" not in value:
        return (None, None)
    left, right = value.split("/", 1)
    return (bytes_value(left), bytes_value(right))


mem_used, mem_limit = split_pair(stats.get("MemUsage"))
net_rx, net_tx = split_pair(stats.get("NetIO"))
block_read, block_write = split_pair(stats.get("BlockIO"))
payload = {
    "readyCpuPercentValue": percent(stats.get("CPUPerc")),
    "readyMemoryUsageBytes": mem_used,
    "readyMemoryLimitBytes": mem_limit,
    "readyMemoryPercentValue": percent(stats.get("MemPerc")),
    "readyPidsCurrent": int(stats["PIDs"]) if str(stats.get("PIDs", "")).isdigit() else None,
    "readyNetworkRxBytes": net_rx,
    "readyNetworkTxBytes": net_tx,
    "readyBlockReadBytes": block_read,
    "readyBlockWriteBytes": block_write,
}
print(json.dumps({key: value for key, value in payload.items() if value is not None}, sort_keys=True))
PY
}
