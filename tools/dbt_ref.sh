#!/bin/sh
# Drives the DBT differential-test pipeline (WP-DBT-04): vectors -> reference runner -> result.
#
#   tools/dbt_ref.sh gen      write the vector file only
#   tools/dbt_ref.sh record   run the runner and pack a golden file ($DBT_REF_DIR/record.golden)
#   tools/dbt_ref.sh fresh    run the runner and compare the result with T0 directly (fresh seed)
#   tools/dbt_ref.sh run      like fresh, but only produce $DBT_REF_DIR/results.raw (arm64 CI job)
#   tools/dbt_ref.sh verify   compare an existing vectors.bin/results.raw with T0 (x86 CI job)
#   tools/dbt_ref.sh smoke    like fresh, small fixed set (a few seconds), part of `make check`
#
# The reference is the real CPU on an arm64 host (oracle "hardware") and qemu-aarch64 elsewhere
# (oracle "qemu", the secondary oracle). Environment:
#   DBT_REF_DIR       work directory (default build/dbt-ref)
#   DBT_REF_SEED      master seed for gen/record (default: the seed of the checked-in golden file)
#   DBT_REF_UNITS     vectors per class weight (default 1000 = about 118k vectors)
#   RITSU_TEST_SEED   seed for fresh/smoke (fresh: default is derived from the clock and printed)
#   DBT_REF_ORACLE    force "qemu" even on arm64 (to cross-check qemu on real hardware)
#   QEMU              qemu-aarch64 binary
#   DBT_REF_REQUIRED  1: missing tools are an error instead of "skip"
set -eu

mode=${1:?usage: dbt_ref.sh gen|record|fresh|run|verify|smoke}
root=$(cd "$(dirname "$0")/.." && pwd)
dir=${DBT_REF_DIR:-$root/build/dbt-ref}
manifest=$root/tests/dbt-ref/Cargo.toml
bin=$root/tests/dbt-ref/target/release/ritsu-dbt-ref
runner=$dir/ritsu-ref-runner
qemu=${QEMU:-qemu-aarch64}
required=${DBT_REF_REQUIRED:-0}
default_seed=0x5249545355303031

skip_or_fail() {
    if [ "$required" = 1 ]; then
        echo "error: $1" >&2
        exit 1
    fi
    echo "skip: $1 (set DBT_REF_REQUIRED=1 to make this an error)"
    exit 0
}

build_tool() {
    cargo build --release --manifest-path "$manifest" >&2
}

mkdir -p "$dir"

if [ "$mode" = gen ]; then
    build_tool
    "$bin" gen --seed "${DBT_REF_SEED:-$default_seed}" --units "${DBT_REF_UNITS:-1000}" --out "$dir/vectors.bin"
    exit 0
fi

if [ "$mode" = verify ]; then
    build_tool
    exec "$bin" check-raw --vectors "$dir/vectors.bin" --raw "$dir/results.raw"
fi

# --- choose the reference -------------------------------------------------------------------
arch=$(uname -m)
if [ "$arch" = aarch64 ] || [ "$arch" = arm64 ]; then
    if [ "${DBT_REF_ORACLE:-}" = qemu ]; then
        command -v "$qemu" >/dev/null 2>&1 || skip_or_fail "$qemu not installed"
        oracle=qemu
        run="$qemu"
    else
        oracle=hardware
        run=""
    fi
else
    command -v "$qemu" >/dev/null 2>&1 || skip_or_fail "$qemu not installed (apt-get install qemu-user)"
    oracle=qemu
    run="$qemu"
fi
if ! command -v clang-18 >/dev/null 2>&1 && ! command -v clang >/dev/null 2>&1 && [ -z "${CC:-}" ]; then
    skip_or_fail "clang not installed (needed to build ritsu-ref-runner)"
fi
command -v ld.lld >/dev/null 2>&1 || command -v lld >/dev/null 2>&1 || skip_or_fail "ld.lld not installed"

build_tool
sh "$root/tests/dbt-ref/runner/build.sh" "$runner"

case "$mode" in
record)
    seed=${DBT_REF_SEED:-$default_seed}
    units=${DBT_REF_UNITS:-1000}
    ;;
fresh | run)
    seed=${RITSU_TEST_SEED:-$(date +%s)}
    units=${DBT_REF_UNITS:-1000}
    ;;
smoke)
    seed=${RITSU_TEST_SEED:-0x534d4f4b45}
    units=${DBT_REF_UNITS:-30}
    ;;
*)
    echo "error: unknown mode $mode" >&2
    exit 2
    ;;
esac
echo "dbt-ref $mode: oracle=$oracle seed=$seed units=$units"
echo "RITSU_TEST_SEED=$seed"

"$bin" gen --seed "$seed" --units "$units" --out "$dir/vectors.bin"
# shellcheck disable=SC2086
$run "$runner" -i "$dir/vectors.bin" -o "$dir/results.raw" -t 600

case "$mode" in
record)
    set -- --meta "runner_source=tests/dbt-ref/runner/ritsu-ref-runner.c"
    if commit=$(git -C "$root" rev-parse HEAD 2>/dev/null); then
        set -- "$@" --meta "commit=$commit"
        # Uncommitted changes to the generator or runner: the recording is not reproducible from the commit alone.
        if [ -n "$(git -C "$root" status --porcelain -- tests/dbt-ref/src tests/dbt-ref/runner 2>/dev/null)" ]; then
            set -- "$@" --meta "dirty=yes"
        fi
    fi
    if [ "$oracle" = qemu ]; then
        set -- "$@" --meta "qemu=$($qemu --version | head -n 1)"
    fi
    cc=${CC:-clang-18}
    command -v "$cc" >/dev/null 2>&1 || cc=clang
    set -- "$@" --meta "cc=$($cc --version | head -n 1)"
    "$bin" pack --vectors "$dir/vectors.bin" --raw "$dir/results.raw" --oracle "$oracle" "$@" --out "$dir/record.golden"
    "$bin" info "$dir/record.golden"
    ;;
run) ;;
fresh | smoke)
    "$bin" check-raw --vectors "$dir/vectors.bin" --raw "$dir/results.raw"
    ;;
esac
