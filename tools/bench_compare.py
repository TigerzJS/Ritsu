#!/usr/bin/env python3
"""Compare a benchmark result with a baseline (docs/process/testing.md section 8).

Both files are result documents written by `ritsu-bench` (bench/README.md, schema 1).

Policy (testing.md section 8):

* kind "deterministic" (sizes, counts): FAIL when worse than the baseline by more than 2 %.
* kind "wall" (throughput, latency): WARN when worse by more than 10 %. On the reference
  machine class (host_id "g5mf", fixed governor) or with --strict-wall the limit is 5 % and the
  verdict is FAIL.
* A different `work_per_call` means the workload changed: the comparison is meaningless and
  fails ("regenerate the baseline"). The same holds for a benchmark that vanished.
* A baseline from a different CPU model is an error on strict hosts and a note elsewhere.
* Results from debug builds or --quick runs are rejected.

"Worse" respects the direction of each metric (`better`: higher/lower). Improvements are
reported, never failed; update the baseline in a PR with a before/after table.

Usage:
    python3 tools/bench_compare.py BASELINE.json RESULT.json [--wall-warn PCT] [--wall-fail PCT]
                                   [--det-fail PCT] [--strict-wall]
    python3 tools/bench_compare.py --merge OUT.json RESULT.json...   # baseline = median of several runs
    python3 tools/bench_compare.py --validate RESULT.json...         # schema check only
    python3 tools/bench_compare.py --self-test

Exit status: 0 = ok (warnings allowed), 1 = regression or invalid comparison, 2 = usage/input error.
Stdlib only.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import unittest
from dataclasses import dataclass, field

DET_FAIL_PCT = 2.0
WALL_WARN_PCT = 10.0
WALL_FAIL_PCT_STRICT = 5.0
STRICT_HOSTS = {"g5mf"}


class InputError(Exception):
    """Unusable input document (exit status 2)."""


@dataclass
class Report:
    """Findings of one comparison."""

    failures: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    rows: list[tuple[str, str, str, str, str]] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return not self.failures


def load(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as f:
            doc = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        raise InputError(f"{path}: {e}") from e
    validate(doc, path)
    return doc


def validate(doc: object, name: str) -> None:
    if not isinstance(doc, dict) or doc.get("schema") != 1:
        raise InputError(f"{name}: not a ritsu-bench result document (schema 1)")
    if not isinstance(doc.get("meta"), dict) or not isinstance(doc.get("results"), list):
        raise InputError(f"{name}: missing meta/results")
    seen = set()
    for r in doc["results"]:
        for key in ("name", "kind", "unit", "better", "value"):
            if key not in r:
                raise InputError(f"{name}: result without {key!r}: {r}")
        if r["kind"] not in ("wall", "deterministic"):
            raise InputError(f"{name}: unknown kind {r['kind']!r} in {r['name']}")
        if r["better"] not in ("higher", "lower"):
            raise InputError(f"{name}: unknown 'better' {r['better']!r} in {r['name']}")
        if not isinstance(r["value"], (int, float)) or isinstance(r["value"], bool):
            raise InputError(f"{name}: non-numeric value in {r['name']}")
        if r["name"] in seen:
            raise InputError(f"{name}: duplicate result {r['name']}")
        seen.add(r["name"])


def median(values: list[float]) -> float:
    v = sorted(values)
    n = len(v)
    return v[n // 2] if n % 2 else (v[n // 2 - 1] + v[n // 2]) / 2


def merge(docs: list[dict]) -> dict:
    """Combine several runs of one machine into one document (per-benchmark median of the medians).

    A single run on a shared machine can be 20-50 % off; a baseline made from several runs is far
    steadier. `spread_pct` of the merged wall results is the run-to-run spread
    (100 * 1.4826 * MAD / median over the run medians), not the in-run spread. Deterministic
    results must be identical in every run, work counts too; anything else is an input error.
    """
    if not docs:
        raise InputError("--merge needs at least one result")
    first = docs[0]
    for d in docs[1:]:
        for key in ("host_id",):
            if d["meta"].get(key) != first["meta"].get(key):
                raise InputError(f"runs disagree on meta.{key}")
        if d["meta"].get("cpu", {}).get("model") != first["meta"].get("cpu", {}).get("model"):
            raise InputError("runs come from different CPU models")
        if sorted(r["name"] for r in d["results"]) != sorted(r["name"] for r in first["results"]):
            raise InputError("runs contain different benchmarks")
    merged = []
    for r in first["results"]:
        same = [next(x for x in d["results"] if x["name"] == r["name"]) for d in docs]
        values = [x["value"] for x in same]
        out = dict(r)
        if r["kind"] == "deterministic":
            if len(set(values)) != 1:
                raise InputError(f"{r['name']}: deterministic value differs between runs: {values}")
        else:
            if len({x.get("work_per_call") for x in same}) != 1:
                raise InputError(f"{r['name']}: work_per_call differs between runs")
            m = median(values)
            mad = median([abs(v - m) for v in values])
            out["value"] = m
            out["mad"] = mad
            out["spread_pct"] = 0.0 if m == 0 else 100.0 * 1.482602218505602 * mad / abs(m)
            out["min"] = min(x["min"] for x in same)
            out["max"] = max(x["max"] for x in same)
            out["samples"] = sum(x.get("samples", 0) for x in same)
        merged.append(out)
    meta = dict(first["meta"])
    meta["merged_runs"] = len(docs)
    meta["merged_from"] = [
        {"commit": d["meta"].get("commit"), "created_utc": d["meta"].get("created_utc")} for d in docs
    ]
    meta["dirty"] = any(d["meta"].get("dirty") for d in docs)
    return {"schema": 1, "meta": meta, "results": merged}


def worse_pct(base: float, new: float, better: str) -> float:
    """Percent by which `new` is worse than `base` (negative = better)."""
    if base == 0:
        return 0.0 if new == 0 else float("inf")
    delta = (new - base) / abs(base) * 100.0
    return -delta if better == "higher" else delta


def compare(
    base: dict,
    new: dict,
    *,
    det_fail: float = DET_FAIL_PCT,
    wall_warn: float = WALL_WARN_PCT,
    wall_fail: float | None = None,
    strict: bool | None = None,
) -> Report:
    """Compare two validated documents. `wall_fail=None` means wall regressions only warn."""
    rep = Report()
    bm, nm = base["meta"], new["meta"]
    host = nm.get("host_id")
    if strict is None:
        strict = host in STRICT_HOSTS
    if strict and wall_fail is None:
        wall_fail = WALL_FAIL_PCT_STRICT

    if bm.get("host_id") != host:
        rep.failures.append(f"host_id differs: baseline {bm.get('host_id')!r}, result {host!r}")
    if nm.get("profile", {}).get("debug_assertions"):
        rep.failures.append("result comes from a debug build; numbers are meaningless")
    if nm.get("config", {}).get("quick") or bm.get("config", {}).get("quick"):
        rep.failures.append("a --quick run cannot be compared")
    if nm.get("dirty"):
        rep.notes.append("result was measured on a dirty working tree (not reproducible from the commit)")

    bcpu = bm.get("cpu", {}).get("model")
    ncpu = nm.get("cpu", {}).get("model")
    cpu_differs = bcpu != ncpu
    if cpu_differs:
        msg = f"CPU differs: baseline {bcpu!r}, result {ncpu!r}"
        if strict:
            rep.failures.append(msg + " (wall-clock comparison invalid on a strict host)")
        else:
            rep.notes.append(msg + "; wall-clock deltas are not meaningful")

    bres = {r["name"]: r for r in base["results"]}
    nres = {r["name"]: r for r in new["results"]}
    for name in sorted(bres.keys() - nres.keys()):
        rep.failures.append(f"{name}: in baseline but missing from result (removed? update the baseline)")
    for name in sorted(nres.keys() - bres.keys()):
        rep.notes.append(f"{name}: new benchmark, not in baseline")

    for name in sorted(bres.keys() & nres.keys()):
        b, n = bres[name], nres[name]
        if b["kind"] != n["kind"] or b["better"] != n["better"] or b["unit"] != n["unit"]:
            rep.failures.append(f"{name}: kind/unit/direction changed; regenerate the baseline")
            continue
        if b["kind"] == "wall" and b.get("work_per_call") != n.get("work_per_call"):
            rep.failures.append(
                f"{name}: workload changed (work_per_call {b.get('work_per_call')} -> "
                f"{n.get('work_per_call')}); regenerate the baseline"
            )
            continue
        w = worse_pct(b["value"], n["value"], b["better"])
        delta = f"{-w:+.1f}%" if b["better"] == "higher" else f"{w:+.1f}%"
        noise = max(b.get("spread_pct", 0.0), n.get("spread_pct", 0.0))
        verdict = "ok"
        if b["kind"] == "deterministic":
            if w > det_fail:
                verdict = "FAIL"
                rep.failures.append(
                    f"{name}: {b['value']} -> {n['value']} {b['unit']} ({w:+.1f}% worse, limit {det_fail}%)"
                )
            elif w < -det_fail:
                verdict = "better"
        else:
            if wall_fail is not None and w > wall_fail and not cpu_differs:
                verdict = "FAIL"
                rep.failures.append(
                    f"{name}: {b['value']:.4g} -> {n['value']:.4g} {b['unit']} "
                    f"({w:.1f}% worse, limit {wall_fail}%, spread {noise:.1f}%)"
                )
            elif w > wall_warn and not cpu_differs:
                verdict = "WARN"
                rep.warnings.append(
                    f"{name}: {b['value']:.4g} -> {n['value']:.4g} {b['unit']} "
                    f"({w:.1f}% worse, warn limit {wall_warn}%, spread {noise:.1f}%)"
                )
            elif w < -wall_warn and not cpu_differs:
                verdict = "better"
        rep.rows.append(
            (name, f"{b['value']:.6g}", f"{n['value']:.6g}", delta, verdict + (f" (spread {noise:.0f}%)" if b["kind"] == "wall" else ""))
        )
    return rep


def render(rep: Report, base: dict, new: dict) -> str:
    out = []
    bm, nm = base["meta"], new["meta"]
    out.append(
        f"baseline: {str(bm.get('commit'))[:12]} {bm.get('cpu', {}).get('model')} ({bm.get('created_utc')})"
    )
    out.append(
        f"result:   {str(nm.get('commit'))[:12]} {nm.get('cpu', {}).get('model')} ({nm.get('created_utc')})"
    )
    width = max((len(r[0]) for r in rep.rows), default=10)
    out.append(f"{'benchmark':<{width}}  {'baseline':>12}  {'result':>12}  {'delta':>8}  verdict")
    for name, b, n, d, v in rep.rows:
        out.append(f"{name:<{width}}  {b:>12}  {n:>12}  {d:>8}  {v}")
    for title, items in (("FAIL", rep.failures), ("WARN", rep.warnings), ("note", rep.notes)):
        for item in items:
            out.append(f"{title}: {item}")
    out.append("result: " + ("OK" if rep.ok else "REGRESSION / INVALID COMPARISON"))
    return "\n".join(out)


def github_annotations(rep: Report) -> list[str]:
    lines = [f"::error title=bench regression::{m}" for m in rep.failures]
    lines += [f"::warning title=bench regression::{m}" for m in rep.warnings]
    return lines


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("baseline", nargs="?")
    ap.add_argument("result", nargs="?")
    ap.add_argument("extra", nargs="*", help=argparse.SUPPRESS)
    ap.add_argument("--wall-warn", type=float, default=WALL_WARN_PCT, metavar="PCT")
    ap.add_argument("--wall-fail", type=float, default=None, metavar="PCT",
                    help="fail (not just warn) on wall-clock regressions above PCT")
    ap.add_argument("--det-fail", type=float, default=DET_FAIL_PCT, metavar="PCT")
    ap.add_argument("--strict-wall", action="store_true",
                    help="treat the host as a strict class (fail at 5 %%, CPU must match)")
    ap.add_argument("--merge", metavar="OUT", help="write the per-benchmark median of the given results to OUT")
    ap.add_argument("--validate", action="store_true", help="only check that the given files are valid results")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args(argv)

    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(SelfTest)
        res = unittest.TextTestRunner(verbosity=1).run(suite)
        return 0 if res.wasSuccessful() else 1
    if args.merge or args.validate:
        files = [f for f in (args.baseline, args.result, *args.extra) if f]
        if not files:
            ap.error("result files are required")
        try:
            docs = [load(f) for f in files]
            if args.merge:
                with open(args.merge, "w", encoding="utf-8") as f:
                    json.dump(merge(docs), f, indent=2)
                    f.write("\n")
                print(f"wrote {args.merge} (median of {len(docs)} runs)")
            else:
                print(f"{len(docs)} result file(s) valid")
        except (InputError, OSError) as e:
            print(f"error: {e}", file=sys.stderr)
            return 2
        return 0
    if not args.baseline or not args.result or args.extra:
        ap.error("BASELINE and RESULT are required (or use --self-test)")
    try:
        base, new = load(args.baseline), load(args.result)
    except InputError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    rep = compare(
        base,
        new,
        det_fail=args.det_fail,
        wall_warn=args.wall_warn,
        wall_fail=args.wall_fail,
        strict=True if args.strict_wall else None,
    )
    print(render(rep, base, new))
    if os.environ.get("GITHUB_ACTIONS") == "true":
        for line in github_annotations(rep):
            print(line)
    return 0 if rep.ok else 1


# ---------------------------------------------------------------------------------------------
# Self-test: synthetic documents with hand-computed expectations.
# ---------------------------------------------------------------------------------------------


def _doc(results, host="ci-x86", cpu="CPU A", **meta):
    m = {"host_id": host, "cpu": {"model": cpu}, "profile": {"debug_assertions": False},
         "config": {"quick": False}, "commit": "abc", "created_utc": "2026-01-01T00:00:00Z", "dirty": False}
    m.update(meta)
    return {"schema": 1, "meta": m, "results": results}


def _wall(name, value, better="higher", work=1000, spread=2.0, unit="M/s"):
    return {"name": name, "kind": "wall", "unit": unit, "better": better, "value": value,
            "work_per_call": work, "spread_pct": spread, "min": value, "max": value, "samples": 15}


def _det(name, value):
    return {"name": name, "kind": "deterministic", "unit": "blocks", "better": "lower", "value": value}


class SelfTest(unittest.TestCase):
    def test_worse_pct_respects_direction(self):
        self.assertAlmostEqual(worse_pct(100, 90, "higher"), 10.0)
        self.assertAlmostEqual(worse_pct(100, 110, "higher"), -10.0)
        self.assertAlmostEqual(worse_pct(100, 110, "lower"), 10.0)
        self.assertAlmostEqual(worse_pct(100, 90, "lower"), -10.0)
        self.assertEqual(worse_pct(0, 0, "lower"), 0.0)
        self.assertEqual(worse_pct(0, 1, "lower"), float("inf"))

    def test_identical_documents_pass(self):
        d = _doc([_wall("a", 100.0), _det("b", 8193)])
        rep = compare(d, d)
        self.assertTrue(rep.ok)
        self.assertEqual((rep.warnings, rep.failures), ([], []))

    def test_wall_regression_warns_above_10_percent_only(self):
        base = _doc([_wall("a", 100.0)])
        self.assertEqual(compare(base, _doc([_wall("a", 91.0)])).warnings, [])  # -9 %
        rep = compare(base, _doc([_wall("a", 89.0)]))  # -11 %
        self.assertTrue(rep.ok)
        self.assertEqual(len(rep.warnings), 1)

    def test_latency_regression_direction(self):
        base = _doc([_wall("l", 100.0, better="lower", unit="us")])
        self.assertEqual(len(compare(base, _doc([_wall("l", 111.0, better="lower", unit="us")])).warnings), 1)
        self.assertEqual(compare(base, _doc([_wall("l", 80.0, better="lower", unit="us")])).warnings, [])

    def test_strict_host_fails_at_5_percent(self):
        base = _doc([_wall("a", 100.0)], host="g5mf")
        rep = compare(base, _doc([_wall("a", 94.0)], host="g5mf"))  # 6 % worse
        self.assertFalse(rep.ok)
        self.assertTrue(compare(base, _doc([_wall("a", 96.0)], host="g5mf")).ok)  # 4 %

    def test_deterministic_limit_is_2_percent(self):
        base = _doc([_det("blocks", 1000)])
        self.assertTrue(compare(base, _doc([_det("blocks", 1020)])).ok)  # exactly 2 %
        self.assertFalse(compare(base, _doc([_det("blocks", 1021)])).ok)
        self.assertTrue(compare(base, _doc([_det("blocks", 900)])).ok)  # improvement

    def test_changed_workload_is_a_failure_not_a_speedup(self):
        rep = compare(_doc([_wall("a", 100.0, work=1000)]), _doc([_wall("a", 200.0, work=500)]))
        self.assertFalse(rep.ok)
        self.assertIn("workload changed", rep.failures[0])

    def test_missing_and_new_benchmarks(self):
        rep = compare(_doc([_wall("a", 1.0), _wall("b", 1.0)]), _doc([_wall("a", 1.0), _wall("c", 1.0)]))
        self.assertFalse(rep.ok)
        self.assertTrue(any("b: in baseline" in f for f in rep.failures))
        self.assertTrue(any("c: new benchmark" in n for n in rep.notes))

    def test_cpu_mismatch_is_a_note_on_shared_runners_and_an_error_on_strict_hosts(self):
        rep = compare(_doc([_wall("a", 100.0)]), _doc([_wall("a", 50.0)], cpu="CPU B"))
        self.assertTrue(rep.ok)
        self.assertEqual(rep.warnings, [])  # delta suppressed: different machine
        self.assertTrue(any("CPU differs" in n for n in rep.notes))
        rep = compare(_doc([_wall("a", 100.0)], host="g5mf"), _doc([_wall("a", 100.0)], host="g5mf", cpu="CPU B"))
        self.assertFalse(rep.ok)

    def test_deterministic_metrics_ignore_cpu_differences(self):
        rep = compare(_doc([_det("b", 100)]), _doc([_det("b", 200)], cpu="CPU B"))
        self.assertFalse(rep.ok)

    def test_debug_and_quick_results_are_rejected(self):
        base = _doc([_wall("a", 1.0)])
        self.assertFalse(compare(base, _doc([_wall("a", 1.0)], profile={"debug_assertions": True})).ok)
        self.assertFalse(compare(base, _doc([_wall("a", 1.0)], config={"quick": True})).ok)

    def test_different_host_ids_are_rejected(self):
        self.assertFalse(compare(_doc([_wall("a", 1.0)]), _doc([_wall("a", 1.0)], host="g5mf")).ok)

    def test_validation_rejects_malformed_documents(self):
        for bad in ({}, {"schema": 2, "meta": {}, "results": []}, {"schema": 1, "meta": {}, "results": [{"name": "x"}]}):
            with self.assertRaises(InputError):
                validate(bad, "t")
        dup = _doc([_wall("a", 1.0), _wall("a", 2.0)])
        with self.assertRaises(InputError):
            validate(dup, "t")

    def test_render_contains_verdicts(self):
        base = _doc([_wall("a", 100.0)])
        new = _doc([_wall("a", 50.0)])
        text = render(compare(base, new), base, new)
        self.assertIn("WARN", text)
        self.assertIn("-50.0%", text)

    def test_merge_takes_the_median_and_reports_run_to_run_spread(self):
        runs = [_doc([_wall("a", v, work=10), _det("b", 7)]) for v in (90.0, 100.0, 130.0)]
        m = merge(runs)
        a = next(r for r in m["results"] if r["name"] == "a")
        self.assertEqual(a["value"], 100.0)
        # deviations 10, 0, 30 -> MAD 10 -> 100 * 1.4826 * 10 / 100
        self.assertAlmostEqual(a["spread_pct"], 14.826022185056018)
        self.assertEqual(m["meta"]["merged_runs"], 3)
        self.assertEqual(next(r for r in m["results"] if r["name"] == "b")["value"], 7)
        validate(m, "merged")
        self.assertTrue(compare(m, runs[1]).ok)

    def test_merge_rejects_inconsistent_runs(self):
        with self.assertRaises(InputError):
            merge([_doc([_det("b", 7)]), _doc([_det("b", 8)])])
        with self.assertRaises(InputError):
            merge([_doc([_wall("a", 1.0, work=1)]), _doc([_wall("a", 1.0, work=2)])])
        with self.assertRaises(InputError):
            merge([_doc([_wall("a", 1.0)]), _doc([_wall("a", 1.0)], cpu="CPU B")])
        with self.assertRaises(InputError):
            merge([_doc([_wall("a", 1.0)]), _doc([_wall("c", 1.0)])])

    def test_median_even_and_odd(self):
        self.assertEqual(median([3.0, 1.0, 2.0]), 2.0)
        self.assertEqual(median([4.0, 1.0, 2.0, 3.0]), 2.5)

    def test_cli_exit_codes(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            ok, bad = os.path.join(d, "ok.json"), os.path.join(d, "bad.json")
            with open(ok, "w", encoding="utf-8") as f:
                json.dump(_doc([_det("b", 100)]), f)
            with open(bad, "w", encoding="utf-8") as f:
                json.dump(_doc([_det("b", 150)]), f)
            out = os.path.join(d, "merged.json")
            old_out, old_err = sys.stdout, sys.stderr
            with open(os.devnull, "w", encoding="utf-8") as nul:
                sys.stdout = sys.stderr = nul
                try:
                    self.assertEqual(main([ok, ok]), 0)
                    self.assertEqual(main([ok, bad]), 1)
                    self.assertEqual(main([ok, os.path.join(d, "missing.json")]), 2)
                    self.assertEqual(main(["--validate", ok, bad]), 0)
                    self.assertEqual(main(["--merge", out, ok, bad]), 2)  # deterministic values differ
                    self.assertEqual(main(["--merge", out, ok, ok]), 0)
                    self.assertEqual(main(["--validate", out]), 0)
                    self.assertEqual(main(["--validate", os.path.join(d, "missing.json")]), 2)
                finally:
                    sys.stdout, sys.stderr = old_out, old_err


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
