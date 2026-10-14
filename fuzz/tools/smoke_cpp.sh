#!/bin/sh
# Time-boxed run of the C++ libFuzzer target fuzz_air_sniff (compiler/air2spirv/fuzz).
# Builds with clang (-DAIR2SPIRV_BUILD_FUZZERS=ON, ASan/UBSan are forced on by CMake), then runs
# for FUZZ_TIME seconds (default 60) on the checked-in seed corpus. The checked-in corpus is
# never modified: new inputs go to a work directory.
#
# Environment: FUZZ_TIME, FUZZ_WORK (see smoke.sh), FUZZ_REQUIRED=1 to fail instead of skipping
# when clang/cmake/ninja are missing, CPP_FUZZ_BUILD_DIR (default build/air2spirv-fuzz).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

time=${FUZZ_TIME:-60}
build=${CPP_FUZZ_BUILD_DIR:-build/air2spirv-fuzz}
src=compiler/air2spirv
target=fuzz_air_sniff

for tool in clang clang++ cmake ninja; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        if [ "${FUZZ_REQUIRED:-0}" = 1 ]; then
            echo "error: $tool is missing" >&2
            exit 1
        fi
        echo "skip: $tool is not installed (needed for the libFuzzer build of $target)"
        exit 0
    fi
done

cmake -S "$src" -B "$build" -G Ninja -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ \
    -DAIR2SPIRV_BUILD_FUZZERS=ON -DCMAKE_BUILD_TYPE=RelWithDebInfo
cmake --build "$build"

cleanup_work=0
if [ -z "${FUZZ_WORK:-}" ]; then
    FUZZ_WORK=$(mktemp -d "${TMPDIR:-/tmp}/ritsu-fuzz.XXXXXX")
    cleanup_work=1
fi
cleanup() {
    if [ "$cleanup_work" = 1 ]; then rm -rf "$FUZZ_WORK"; fi
}
trap cleanup EXIT

work="$FUZZ_WORK/$target"
mkdir -p "$work" "fuzz/artifacts/$target"
dirs="$work"
[ -d "$src/fuzz/regressions/$target" ] && dirs="$dirs $src/fuzz/regressions/$target"
dirs="$dirs $src/fuzz/corpus/$target"

echo "=== $target (${time}s) ==="
# shellcheck disable=SC2086  # $dirs is a list of paths without spaces
"$build/fuzz/$target" "-max_total_time=$time" -timeout=5 -rss_limit_mb=2048 \
    "-artifact_prefix=$root/fuzz/artifacts/$target/" $dirs
