#!/bin/sh
# Time-boxed fuzz run of the Rust targets (docs/process/testing.md section 5.4).
#
#   fuzz/tools/smoke.sh                 all targets, FUZZ_TIME seconds each (default 60)
#   fuzz/tools/smoke.sh fuzz_mmio_bus   only the named targets
#
# Environment:
#   FUZZ_TIME       seconds per target (default 60)
#   FUZZ_TOOLCHAIN  rustup toolchain for cargo-fuzz (default nightly)
#   FUZZ_WORK       directory for the growing corpora (default: fresh temp dir, removed on exit
#                   unless FUZZ_WORK is set; CI sets it so the cache/artifact step can pick it up)
#   FUZZ_REQUIRED   set to 1 to fail instead of skipping when nightly/cargo-fuzz are missing
#
# The checked-in corpus is never modified: libFuzzer writes new inputs into the work directory
# and only reads fuzz/corpus/<target> and fuzz/regressions/<target> (reproducers of earlier
# findings are replayed first, so a regression fails the run immediately).
# A crash, timeout or out-of-memory exits non-zero; artifacts land in fuzz/artifacts/<target>/.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

time=${FUZZ_TIME:-60}
toolchain=${FUZZ_TOOLCHAIN:-nightly}

if ! cargo "+$toolchain" fuzz --version >/dev/null 2>&1; then
    if [ "${FUZZ_REQUIRED:-0}" = 1 ]; then
        echo "error: cargo-fuzz or the '$toolchain' toolchain is missing" >&2
        exit 1
    fi
    echo "skip: cargo-fuzz or the '$toolchain' toolchain is not installed" \
        "(rustup toolchain install $toolchain; cargo install cargo-fuzz)"
    exit 0
fi

if [ "$#" -gt 0 ]; then
    targets="$*"
else
    targets=$(cargo "+$toolchain" fuzz list)
fi

cleanup_work=0
if [ -z "${FUZZ_WORK:-}" ]; then
    FUZZ_WORK=$(mktemp -d "${TMPDIR:-/tmp}/ritsu-fuzz.XXXXXX")
    cleanup_work=1
fi
cleanup() {
    if [ "$cleanup_work" = 1 ]; then rm -rf "$FUZZ_WORK"; fi
}
trap cleanup EXIT

status=0
for t in $targets; do
    echo "=== $t (${time}s) ==="
    work="$FUZZ_WORK/$t"
    mkdir -p "$work"
    dirs="$work"
    [ -d "fuzz/regressions/$t" ] && dirs="$dirs fuzz/regressions/$t"
    [ -d "fuzz/corpus/$t" ] && dirs="$dirs fuzz/corpus/$t"
    # shellcheck disable=SC2086  # $dirs is a list of paths without spaces
    if ! cargo "+$toolchain" fuzz run "$t" $dirs -- \
        "-max_total_time=$time" -timeout=5 -rss_limit_mb=2048; then
        echo "FAILED: $t (see fuzz/artifacts/$t/)" >&2
        status=1
    fi
done
exit $status
