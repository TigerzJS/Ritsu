#!/usr/bin/env python3
"""Deterministic instruction-count benchmarks (docs/process/testing.md section 8, bench/README.md).

Wall-clock numbers on shared CI runners are too noisy to gate on (measured spread of run medians:
8-66 %). The number of host instructions a workload executes is not: it depends on the code and
the compiler, not on the machine's load. This script measures it with `valgrind --tool=callgrind`
and writes a result document in the usual format (schema 1, kind "deterministic"), so that
`tools/bench_compare.py` gates it with the 2 % limit.

The counts include libc (memcpy, malloc). The child processes run with fixed glibc tunables
(LIBC_TUNABLES) so that the CPU's feature bits do not change which memcpy variant is used; the glibc
version is recorded (`meta.config.libc`) because a different glibc is a different baseline.

Method, per workload (`ritsu-bench --icount-run NAME --reps N`, which builds the workload once and
calls its closure N times):

    Ir(N)  = S + N * c        S = process start-up + workload set-up, c = cost of one call
    Ir(2N) = S + 2N * c
    c      = (Ir(2N) - Ir(N)) / N        S cancels

The result is `c / work_per_call`: host instructions per work unit (per guest instruction for the
T0 interpreter, per byte for the protocol decoders, per block for the IR passes, per pop+push
pair for the virtqueue). For T0 this is "host instructions per guest instruction", an early
indicator for Q-PERF-1.

Usage:
    python3 tools/bench_icount.py --bin bench/target/release/ritsu-bench --host-id icount-x86_64 \\
        --output bench/results/icount-x86_64.json [--filter TEXT] [--repeat R] [--jobs J]
    python3 tools/bench_icount.py --self-test

Exit status: 0 = ok, 1 = measurement failed or not reproducible, 2 = usage error.
Stdlib only; needs `valgrind` on the PATH (or --valgrind).
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import threading
import sys
import tempfile
import unittest
from typing import Callable

# Maximum relative deviation between repeated measurements of one workload before the run is
# declared not reproducible. Measured on the reference host: 0 % (see bench/README.md); the limit
# leaves room for allocator/hash-seed effects, far below the 2 % gate.
MAX_SPREAD_PCT = 0.5

HOST_INSN_UNIT = {
    "instructions": "host_insn/guest_insn",
    "blocks": "host_insn/block",
    "bytes": "host_insn/byte",
    "pop+push pairs": "host_insn/pair",
}

# glibc picks memcpy/memset/... variants from the CPU's feature bits and cache sizes at start-up
# (ifunc). Measured on one host with different masks: AVX2/ERMS/FSRM change the instruction count
# of memcpy-heavy workloads by 1 to 28 % (T0 and the virtqueue are unaffected), SSSE3/SSE4/
# AVX_Fast_Unaligned_Load by up to 23 %. A baseline made on one CPU would then fail on another, so
# the child processes get the same tunables everywhere: the lowest variant (every x86-64 CPU can
# follow a mask) and fixed cache thresholds. With these, further masks and different CPU features
# changed the counts by less than 0.002 % (bench/README.md).
LIBC_TUNABLES = (
    "glibc.cpu.hwcaps=-AVX2,-AVX,-AVX512F,-AVX512VL,-AVX512BW,-AVX512DQ,-AVX512CD,-ERMS,-FSRM,"
    "-BMI1,-BMI2,-RTM,-LZCNT,-MOVBE,-POPCNT,-SSE4_1,-SSE4_2,-SSSE3,-AVX_Fast_Unaligned_Load"
    ":glibc.cpu.x86_non_temporal_threshold=0x300000"
    ":glibc.cpu.x86_data_cache_size=0x8000"
    ":glibc.cpu.x86_shared_cache_size=0x400000"
)

VALGRIND_FLAGS = [
    "--tool=callgrind",
    "--cache-sim=no",
    "--branch-sim=no",
    "--dump-instr=no",
    "--collect-jumps=no",
    "--collect-systime=no",
    "--compress-strings=no",
    "--compress-pos=no",
]


class MeasureError(Exception):
    """A measurement could not be taken or is inconsistent (exit status 1)."""


# A runner executes the workload `name` with `reps` calls and returns (host instructions, work
# units per call). Injected so the arithmetic can be tested without valgrind.
Runner = Callable[[str, int], "tuple[int, int]"]


def parse_callgrind_total(text: str) -> int:
    """Total instruction count (`Ir`) of a callgrind output file.

    Callgrind writes `summary: N` (and `totals: N`) lines; with several parts (threads) the
    `totals` line is the sum over all of them.
    """
    totals = re.findall(r"^totals:\s+(\d+)\s*$", text, re.M)
    summary = re.findall(r"^summary:\s+(\d+)\s*$", text, re.M)
    if totals:
        return int(totals[-1])
    if summary:
        return sum(int(s) for s in summary)
    raise MeasureError("callgrind output has neither a 'totals:' nor a 'summary:' line")


def parse_work(stdout: str) -> int:
    m = re.search(r"^work_per_call=(\d+)\s*$", stdout, re.M)
    if not m:
        raise MeasureError(f"workload did not report work_per_call; stdout was: {stdout!r}")
    work = int(m.group(1))
    if work <= 0:
        raise MeasureError("workload reported zero work")
    return work


def make_runner(binary: str, valgrind: str, tmpdir: str) -> Runner:
    counter = iter(range(1_000_000))
    lock = threading.Lock()

    def run(name: str, reps: int) -> tuple[int, int]:
        with lock:
            out = os.path.join(tmpdir, f"cg.{next(counter)}.out")
        cmd = [valgrind, *VALGRIND_FLAGS, f"--callgrind-out-file={out}", binary,
               "--icount-run", name, "--reps", str(reps)]
        env = dict(os.environ, GLIBC_TUNABLES=LIBC_TUNABLES)
        proc = subprocess.run(cmd, capture_output=True, text=True, check=False, env=env)
        if proc.returncode != 0:
            raise MeasureError(f"{name} (reps {reps}) failed with status {proc.returncode}:\n{proc.stderr[-2000:]}")
        try:
            with open(out, encoding="utf-8") as f:
                total = parse_callgrind_total(f.read())
        finally:
            if os.path.exists(out):
                os.remove(out)
        return total, parse_work(proc.stdout)

    return run


def measure_once(run: Runner, name: str, reps: int) -> dict:
    """One difference measurement: instructions per call and per work unit."""
    ir_n, work_n = run(name, reps)
    ir_2n, work_2n = run(name, 2 * reps)
    if work_n != work_2n:
        raise MeasureError(f"{name}: work per call differs between runs ({work_n} vs {work_2n})")
    delta = ir_2n - ir_n
    if delta <= 0:
        raise MeasureError(f"{name}: Ir(2N)={ir_2n} is not larger than Ir(N)={ir_n}")
    per_call = delta / reps
    return {
        "instr_per_call": per_call,
        "work_per_call": work_n,
        "value": per_call / work_n,
        "ir_n": ir_n,
        "ir_2n": ir_2n,
        # What the difference removed: start-up, set-up and the first call's deviation from the
        # steady state (can be negative). Diagnostic only, never gated.
        "fixed_ir": ir_n - per_call * reps,
    }


def spread_pct(values: list[float]) -> float:
    lo, hi = min(values), max(values)
    return 0.0 if hi == 0 else 100.0 * (hi - lo) / hi


def median(values: list[float]) -> float:
    v = sorted(values)
    n = len(v)
    return v[n // 2] if n % 2 else (v[n // 2 - 1] + v[n // 2]) / 2


def measure(run: Runner, name: str, reps: int, repeat: int, max_spread: float) -> dict:
    """`repeat` independent difference measurements; the median is the value."""
    ms = [measure_once(run, name, reps) for _ in range(repeat)]
    if len({m["work_per_call"] for m in ms}) != 1:
        raise MeasureError(f"{name}: work per call differs between repetitions")
    values = [m["value"] for m in ms]
    out = dict(sorted(ms, key=lambda m: m["value"])[len(ms) // 2])
    out["value"] = median(values)
    out["repeats"] = repeat
    out["repeat_spread_pct"] = spread_pct(values)
    if out["repeat_spread_pct"] > max_spread:
        raise MeasureError(
            f"{name}: not reproducible, repetitions differ by {out['repeat_spread_pct']:.3f} % "
            f"(limit {max_spread} %): {values}"
        )
    return out


def result_entry(name: str, q_perf: str, work_unit: str, reps: int, m: dict) -> dict:
    short = name[len("bench_"):] if name.startswith("bench_") else name
    entry = {
        "name": f"icount_{short}",
        "kind": "deterministic",
        "q_perf": q_perf,
        "unit": HOST_INSN_UNIT.get(work_unit, f"host_insn/{work_unit}"),
        "better": "lower",
        "value": m["value"],
        "instr_per_call": m["instr_per_call"],
        "work_per_call": m["work_per_call"],
        "work_unit": work_unit,
        "reps": reps,
        "ir_reps": m["ir_n"],
        "ir_2reps": m["ir_2n"],
        "fixed_ir": m["fixed_ir"],
    }
    if m["repeats"] > 1:
        entry["repeats"] = m["repeats"]
        entry["repeat_spread_pct"] = m["repeat_spread_pct"]
    return entry


def parse_list(text: str) -> list[tuple[str, int, str, str]]:
    rows = []
    for line in text.splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) != 4 or not parts[1].isdigit() or int(parts[1]) == 0:
            raise MeasureError(f"unexpected --icount-list line: {line!r}")
        rows.append((parts[0], int(parts[1]), parts[2], parts[3]))
    return rows


def libc_version() -> str | None:
    """`glibc 2.39` (getconf), None if unavailable."""
    try:
        out = subprocess.run(["getconf", "GNU_LIBC_VERSION"], capture_output=True, text=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        return None
    return out.stdout.strip() or None


def build_document(meta: dict, entries: list[dict], valgrind_version: str, repeat: int, filt: str | None) -> dict:
    meta = dict(meta)
    config = dict(meta.get("config", {}))
    config.update({
        "mode": "icount",
        "tool": valgrind_version,
        "libc": libc_version(),
        "libc_tunables": LIBC_TUNABLES,
        "method": "callgrind Ir, difference of N and 2N closure calls",
        "repeat": repeat,
        "filter": filt,
    })
    meta["config"] = config
    return {"schema": 1, "meta": meta, "results": entries}


def render_table(entries: list[dict]) -> str:
    width = max((len(e["name"]) for e in entries), default=10)
    lines = [f"{'workload':<{width}}  {'host insn/unit':>15}  {'unit':<22}  {'insn/call':>14}  {'work/call':>10}  {'fixed Ir':>12}"]
    for e in entries:
        lines.append(
            f"{e['name']:<{width}}  {e['value']:>15.4f}  {e['unit']:<22}  {e['instr_per_call']:>14.0f}  "
            f"{e['work_per_call']:>10}  {e['fixed_ir']:>12.0f}"
        )
    return "\n".join(lines)


def check_binary_meta(meta: dict) -> None:
    prof = meta.get("profile", {})
    if prof.get("debug_assertions") or prof.get("name") not in ("release", None):
        raise MeasureError("the benchmark binary is not a release build; instruction counts would be meaningless")


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--bin", help="path of the release build of ritsu-bench")
    ap.add_argument("--host-id", default=os.environ.get("RITSU_BENCH_HOST_ID", "icount-x86_64"))
    ap.add_argument("--output", help="result document (default: stdout)")
    ap.add_argument("--filter", help="only workloads whose name contains this text")
    ap.add_argument("--repeat", type=int, default=1, help="independent measurements per workload (median)")
    ap.add_argument("--max-spread", type=float, default=MAX_SPREAD_PCT, metavar="PCT",
                    help="fail if repetitions of one workload differ by more (default %(default)s)")
    ap.add_argument("--jobs", type=int, default=min(4, os.cpu_count() or 1))
    ap.add_argument("--valgrind", default="valgrind")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args(argv)

    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(SelfTest)
        return 0 if unittest.TextTestRunner(verbosity=1).run(suite).wasSuccessful() else 1
    if not args.bin:
        ap.error("--bin is required (or use --self-test)")
    if args.repeat < 1 or args.jobs < 1:
        ap.error("--repeat and --jobs must be positive")
    if shutil.which(args.valgrind) is None:
        print(f"error: {args.valgrind} not found; install valgrind", file=sys.stderr)
        return 2

    try:
        listing = subprocess.run([args.bin, "--icount-list"], capture_output=True, text=True, check=True).stdout
        meta = json.loads(subprocess.run([args.bin, "--icount-meta", "--host-id", args.host_id],
                                         capture_output=True, text=True, check=True).stdout)
        vg_version = subprocess.run([args.valgrind, "--version"], capture_output=True, text=True,
                                    check=True).stdout.strip()
        check_binary_meta(meta)
        rows = [r for r in parse_list(listing) if args.filter is None or args.filter in r[0]]
        if not rows:
            raise MeasureError("no workload matches the filter")
        with tempfile.TemporaryDirectory(prefix="ritsu-icount-") as tmp:
            run = make_runner(args.bin, args.valgrind, tmp)
            with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
                futures = [pool.submit(measure, run, n, reps, args.repeat, args.max_spread) for n, reps, _, _ in rows]
                measured = [f.result() for f in futures]
    except (MeasureError, subprocess.CalledProcessError, OSError, json.JSONDecodeError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1

    entries = [result_entry(n, q, wu, reps, m) for (n, reps, q, wu), m in zip(rows, measured)]
    print(render_table(entries), file=sys.stderr)
    doc = build_document(meta, entries, vg_version, args.repeat, args.filter)
    text = json.dumps(doc, indent=2) + "\n"
    if args.output:
        os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
        with open(args.output, "w", encoding="utf-8") as f:
            f.write(text)
        print(f"wrote {args.output}", file=sys.stderr)
    else:
        sys.stdout.write(text)
    return 0


# ---------------------------------------------------------------------------------------------
# Self-test: hand-computed expectations, fake runners instead of valgrind.
# ---------------------------------------------------------------------------------------------

CG_SAMPLE = """# callgrind format
version: 1
creator: callgrind-3.22.0
events: Ir
summary: 239507632

fn=(1) main
12 100
totals: 239507632
"""


def fake_runner(setup: int, per_call: int, work: int, jitter: Callable[[int], int] = lambda reps: 0) -> Runner:
    def run(name: str, reps: int) -> tuple[int, int]:
        return setup + reps * per_call + jitter(reps), work

    return run


class SelfTest(unittest.TestCase):
    def test_parse_callgrind_total(self):
        self.assertEqual(parse_callgrind_total(CG_SAMPLE), 239507632)
        self.assertEqual(parse_callgrind_total("summary: 5\n"), 5)
        self.assertEqual(parse_callgrind_total("summary: 5\nsummary: 7\n"), 12)  # several parts
        with self.assertRaises(MeasureError):
            parse_callgrind_total("events: Ir\n")

    def test_parse_work(self):
        self.assertEqual(parse_work("noise\nwork_per_call=1000002\n"), 1000002)
        for bad in ("", "work_per_call=0\n", "work_per_call=abc\n"):
            with self.assertRaises(MeasureError):
                parse_work(bad)

    def test_difference_removes_setup_cost(self):
        # Hand-computed: S = 1000, c = 250, N = 4: Ir(4) = 2000, Ir(8) = 3000, c = 1000 / 4.
        m = measure_once(fake_runner(1000, 250, 100), "w", 4)
        self.assertEqual((m["ir_n"], m["ir_2n"]), (2000, 3000))
        self.assertEqual(m["instr_per_call"], 250)
        self.assertEqual(m["value"], 2.5)  # 250 insn / 100 work units
        self.assertEqual(m["fixed_ir"], 1000)  # exactly the set-up cost
        # The value does not depend on the set-up cost.
        self.assertEqual(measure_once(fake_runner(123_456_789, 250, 100), "w", 4)["value"], 2.5)

    def test_rejects_inconsistent_measurements(self):
        calls = iter([(100, 5), (200, 6)])
        with self.assertRaises(MeasureError):  # work differs between N and 2N
            measure_once(lambda n, r: next(calls), "w", 1)
        with self.assertRaises(MeasureError):  # 2N not larger than N
            measure_once(fake_runner(10, 0, 1), "w", 1)

    def test_repeat_takes_median_and_enforces_reproducibility(self):
        seq = iter([0, 0, 0, 0, 0, 0])  # identical repetitions: spread 0
        m = measure(fake_runner(500, 100, 10, lambda r: next(seq)), "w", 2, 3, 0.5)
        self.assertEqual(m["value"], 10.0)
        self.assertEqual(m["repeat_spread_pct"], 0.0)
        self.assertEqual(m["repeats"], 3)
        # Repetition 2 measures 2 % more per call than repetition 1: 100 -> 102 per call.
        counts = iter([300, 500, 300, 504, 300, 500])  # Ir(N), Ir(2N) per repetition, N = 2
        runs = {"i": 0}

        def noisy(name, reps):
            runs["i"] += 1
            return next(counts), 10

        with self.assertRaises(MeasureError):
            measure(noisy, "w", 2, 3, 0.5)  # values 10.0, 10.2, 10.0: 1.96 % spread
        counts = iter([300, 500, 300, 504, 300, 500])
        m = measure(noisy, "w", 2, 3, 5.0)
        self.assertEqual(m["value"], 10.0)  # median
        self.assertAlmostEqual(m["repeat_spread_pct"], 100 * 0.2 / 10.2)

    def test_spread_and_median(self):
        self.assertEqual(spread_pct([100.0, 100.0]), 0.0)
        self.assertAlmostEqual(spread_pct([99.0, 100.0]), 1.0)
        self.assertEqual(median([3.0, 1.0, 2.0]), 2.0)
        self.assertEqual(median([4.0, 1.0]), 2.5)

    def test_result_entry_is_a_deterministic_result_in_the_common_format(self):
        m = measure_once(fake_runner(1000, 250, 100), "bench_t0_mix", 4)
        m.update(repeats=1, repeat_spread_pct=0.0)
        e = result_entry("bench_t0_mix", "Q-PERF-1", "instructions", 4, m)
        self.assertEqual(e["name"], "icount_t0_mix")
        self.assertEqual((e["kind"], e["better"], e["unit"]), ("deterministic", "lower", "host_insn/guest_insn"))
        self.assertEqual(e["value"], 2.5)
        self.assertEqual(e["work_per_call"], 100)
        self.assertNotIn("repeats", e)
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import bench_compare  # same directory

        doc = build_document({"host_id": "h", "config": {"quick": False}}, [e], "valgrind-3.22.0", 1, None)
        bench_compare.validate(doc, "icount")
        self.assertEqual(doc["meta"]["config"]["mode"], "icount")
        self.assertFalse(doc["meta"]["config"]["quick"])

    def test_libc_tunables_are_well_formed(self):
        entries = LIBC_TUNABLES.split(":")
        keys = [e.split("=")[0] for e in entries]
        self.assertEqual(len(keys), len(set(keys)), "a repeated tunable would replace the first")
        self.assertIn("glibc.cpu.hwcaps", keys)
        hw = dict(e.split("=") for e in entries)["glibc.cpu.hwcaps"].split(",")
        self.assertTrue(all(h.startswith("-") for h in hw), "only masks: any CPU can follow them")
        self.assertIn("-AVX2", hw)
        self.assertIn("-ERMS", hw)

    def test_parse_list(self):
        rows = parse_list("bench_a\t2\tQ-PERF-1\tinstructions\nbench_b\t1\tQ-PERF-3\tbytes\n")
        self.assertEqual(rows[0], ("bench_a", 2, "Q-PERF-1", "instructions"))
        self.assertEqual(len(rows), 2)
        for bad in ("bench_a\t0\tq\tw\n", "bench_a\tx\tq\tw\n", "bench_a\t1\n"):
            with self.assertRaises(MeasureError):
                parse_list(bad)

    def test_debug_binary_is_rejected(self):
        check_binary_meta({"profile": {"name": "release", "debug_assertions": False}})
        with self.assertRaises(MeasureError):
            check_binary_meta({"profile": {"name": "debug", "debug_assertions": True}})

    def test_render_table_mentions_every_workload(self):
        m = measure_once(fake_runner(1000, 250, 100), "w", 4)
        m.update(repeats=1, repeat_spread_pct=0.0)
        text = render_table([result_entry("bench_x", "Q-PERF-1", "bytes", 4, m)])
        self.assertIn("icount_x", text)
        self.assertIn("2.5000", text)

    @unittest.skipIf(shutil.which("valgrind") is None, "valgrind not installed")
    def test_callgrind_end_to_end_on_a_tiny_program(self):
        # `true` run under callgrind: only checks that the output is parsed; an
        # instruction count above zero is the independent expectation.
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "o")
            subprocess.run(["valgrind", *VALGRIND_FLAGS, f"--callgrind-out-file={out}", "true"],
                           capture_output=True, check=True)
            with open(out, encoding="utf-8") as f:
                self.assertGreater(parse_callgrind_total(f.read()), 1000)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
