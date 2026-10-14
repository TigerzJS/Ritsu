#!/usr/bin/env python3
"""Classify libFuzzer artifacts of air2spirv by where they happen (WP-SC-02).

LLVM 18's bitcode reader has memory-safety bugs that hostile bitcode reaches within seconds of
fuzzing (docs/subsystems/shader-compiler.md §9, §11.4). They cannot be fixed in air2spirv; the
answer is process isolation (WP-ARCH-04). So a fuzz run of fuzz_air_bitcode collects artifacts
with -ignore_crashes, and this script replays each one and sorts it by the first stack frame
that is neither sanitizer/libFuzzer/libc runtime nor error-reporting plumbing:

    air2spirv  the frame is air2spirv code (namespace air2spirv, compiler/air2spirv sources,
               the fuzz target's invariants)               -> failure, exit status 1
    llvm       the frame is inside LLVM                     -> known, reported
    no-repro   replay shows no finding (nondeterministic)   -> reported
    unknown    no usable stack                              -> failure (cannot be ruled out)

Usage: triage.py <fuzzer> <artifact-or-directory>...   (*.md files in directories are skipped)
"""

import os
import re
import subprocess
import sys

FRAME = re.compile(r"^\s*#(\d+) 0x[0-9a-f]+ in (.*)$")
KIND = re.compile(
    r"ERROR: (AddressSanitizer: [\w-]+|LeakSanitizer|UndefinedBehaviorSanitizer|"
    r"libFuzzer: [\w -]+)|runtime error:|unrecoverable [\w ]+ inside LLVM|out-of-memory"
)

# Runtime and error-reporting frames that say nothing about where the problem is.
SKIP = re.compile(
    r"__sanitizer|__asan|__lsan|__ubsan|__interceptor|asan_|operator new|operator delete|"
    r"\bmalloc\b|\bcalloc\b|\brealloc\b|\bfree\b|fuzzer::|libFuzzer|"
    r"__GI_|\babort\b|\braise\b|pthread_kill|__libc|_start\b|"
    r"report_and_abort|fatal_error_handler|bad_alloc_handler|"
    r"llvm::report_fatal_error|llvm::report_bad_alloc_error|llvm::llvm_unreachable_internal|"
    r"safe_malloc|safe_realloc|allocate_buffer|grow_pod|SmallVectorBase|std::__new_allocator|"
    r"std::allocator|std::vector<.*>::_M_|std::__cxx11|std::_Vector_base"
)
OURS = re.compile(r"air2spirv::|/compiler/air2spirv/|invariant_failed|LLVMFuzzerTestOneInput")
LLVM = re.compile(r"llvm::|BitcodeReader|MetadataLoader|BitstreamCursor|\.cpp\.o\b|/llvm-18/")


def stacks_of(report):
    """All stacks of the report, each a list of frames (function + location text)."""
    stacks = []
    for line in report.splitlines():
        m = FRAME.match(line)
        if m:
            if m.group(1) == "0" or not stacks:
                stacks.append([])
            stacks[-1].append(m.group(2))
    return stacks


def classify_stack(frames):
    for frame in frames:
        if SKIP.search(frame):
            continue
        if OURS.search(frame):
            return "air2spirv", frame
        if LLVM.search(frame):
            return "llvm", frame
        return "unknown", frame
    return "unknown", ""


def classify(report):
    kind_match = KIND.search(report)
    if not kind_match:
        return "no-repro", "-", ""
    kind = kind_match.group(0)
    stacks = stacks_of(report)
    if not stacks:
        return "unknown", kind, ""
    # The first stack is the finding. Later stacks of sanitizer errors describe the memory
    # ("allocated by", "freed by") and may name air2spirv as the allocator of an input buffer
    # that LLVM overreads, so they do not count. A leak report lists one stack per leak; every
    # leak counts.
    candidates = stacks if kind == "ERROR: LeakSanitizer" else stacks[:1]
    verdicts = [classify_stack(frames) for frames in candidates]
    for verdict, frame in verdicts:
        if verdict == "air2spirv":
            return verdict, kind, frame
    for verdict, frame in verdicts:
        if verdict == "unknown":
            return verdict, kind, frame
    return verdicts[0][0], kind, verdicts[0][1]


def replay(fuzzer, path):
    env = dict(os.environ)
    env.setdefault("ASAN_OPTIONS", "detect_leaks=1:symbolize=1")
    try:
        done = subprocess.run(
            [fuzzer, "-timeout=30", "-rss_limit_mb=2048", path],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=env,
            timeout=120,
            check=False,
        )
        return done.stdout.decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        return "ERROR: libFuzzer: timeout (replay exceeded 120 s)"


def short(frame):
    frame = re.sub(r"\s*\((/|BuildId)[^)]*\)", "", frame)
    frame = re.sub(r"\(anonymous namespace\)::", "", frame)
    return frame[:110]


def main():
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    fuzzer = sys.argv[1]
    paths = []
    for arg in sys.argv[2:]:
        if os.path.isdir(arg):
            paths.extend(
                sorted(os.path.join(arg, n) for n in os.listdir(arg) if not n.endswith(".md"))
            )
        elif os.path.exists(arg):
            paths.append(arg)
    counts = {}
    failures = 0
    for path in paths:
        verdict, kind, frame = classify(replay(fuzzer, path))
        counts[verdict] = counts.get(verdict, 0) + 1
        if verdict in ("air2spirv", "unknown"):
            failures += 1
        print("%-9s %-40s %s  [%s]" % (verdict, os.path.basename(path)[:40], short(frame), kind))
    summary = ", ".join("%s=%d" % (k, counts[k]) for k in sorted(counts)) or "no artifacts"
    print("triage: %s" % summary)
    if failures:
        print("triage: %d finding(s) in air2spirv or unclassified: FAIL" % failures)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
