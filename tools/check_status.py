#!/usr/bin/env python3
"""Validate status/*.json, the machine-readable project status behind the live roadmap page.

Rules: every map has id/title/groups, every item has a unique id within its map and a status in
STATUSES, meta.json has goal, milestones (M0..M8, unique ids) and at most MAX_RUNS runs.
Exit 0 when valid, 1 on violations, 2 on usage/IO errors. Python stdlib only.
"""
import json
import sys
from pathlib import Path

STATUSES = {"open", "wip", "done", "blocked"}
MAX_RUNS = 20
ROOT = Path(__file__).resolve().parent.parent / "status"


def check_map(path, data, errors):
    for key in ("id", "title", "groups"):
        if key not in data:
            errors.append(f"{path.name}: missing '{key}'")
    if path.stem != f"map-{data.get('id')}":
        errors.append(f"{path.name}: file name must be map-<id>.json (id={data.get('id')!r})")
    seen = set()
    for g in data.get("groups", []):
        if not g.get("name"):
            errors.append(f"{path.name}: group without name")
        for it in g.get("items", []):
            iid = it.get("id")
            if not iid:
                errors.append(f"{path.name}/{g.get('name')}: item without id")
                continue
            if iid in seen:
                errors.append(f"{path.name}: duplicate id {iid!r}")
            seen.add(iid)
            if it.get("status") not in STATUSES:
                errors.append(f"{path.name}: {iid}: status {it.get('status')!r} not in {sorted(STATUSES)}")
            w = it.get("weight", 1)
            if not isinstance(w, (int, float)) or w <= 0:
                errors.append(f"{path.name}: {iid}: weight must be > 0")
    if not seen:
        errors.append(f"{path.name}: no items")


def check_meta(path, data, errors):
    if not data.get("goal"):
        errors.append("meta.json: missing goal")
    ids = [m.get("id") for m in data.get("milestones", [])]
    if ids != [f"M{i}" for i in range(9)]:
        errors.append(f"meta.json: milestones must be M0..M8 in order, got {ids}")
    for m in data.get("milestones", []):
        if m.get("status") not in STATUSES:
            errors.append(f"meta.json: {m.get('id')}: bad status {m.get('status')!r}")
    if len(data.get("runs", [])) > MAX_RUNS:
        errors.append(f"meta.json: more than {MAX_RUNS} runs, prune old ones")


def main():
    if not ROOT.is_dir():
        print(f"status dir not found: {ROOT}", file=sys.stderr)
        return 2
    errors = []
    maps = sorted(ROOT.glob("map-*.json"))
    if not maps:
        errors.append("no status/map-*.json files")
    for p in maps + [ROOT / "meta.json"]:
        try:
            data = json.loads(p.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as e:
            errors.append(f"{p.name}: {e}")
            continue
        (check_meta if p.name == "meta.json" else check_map)(p, data, errors)
    if errors:
        print("status check failed:", *errors, sep="\n  ")
        return 1
    print(f"status ok: {len(maps)} maps + meta.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
