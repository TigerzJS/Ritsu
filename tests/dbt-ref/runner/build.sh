#!/bin/sh
# Builds the static aarch64-Linux reference runner (no libc).
# Usage: build.sh <output-file>
# Environment: CC (default clang-18, or clang), RITSU_RUNNER_CFLAGS.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:?usage: build.sh <output-file>}
cc=${CC:-}
if [ -z "$cc" ]; then
    if command -v clang-18 >/dev/null 2>&1; then cc=clang-18; else cc=clang; fi
fi
mkdir -p "$(dirname "$out")"
# shellcheck disable=SC2086
"$cc" --target=aarch64-linux-gnu -static -nostdlib -ffreestanding -fno-builtin -fno-pic \
    -fno-stack-protector -fno-asynchronous-unwind-tables -mno-outline-atomics -fuse-ld=lld \
    -O2 -Wall -Wextra -Werror -Wl,-e,_start -Wl,-z,norelro -Wl,--build-id=none \
    ${RITSU_RUNNER_CFLAGS:-} -o "$out" "$here/ritsu-ref-runner.c"
