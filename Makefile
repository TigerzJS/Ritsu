# Ritsu - top-level build/check entry point (see ADR-0003, docs/process/testing.md).
# Plain GNU make / POSIX make compatible; recipes need a POSIX shell.

CARGO        ?= cargo
PYTHON       ?= python3
CMAKE        ?= cmake
CTEST        ?= ctest
CLANG_FORMAT ?= clang-format
CPP_SRC_DIR  ?= compiler/air2spirv
CPP_BUILD_DIR ?= build/air2spirv
CPP_BUILD_TYPE ?= RelWithDebInfo
CPP_CMAKE_ARGS ?=
# Set CC/CXX in the environment to pick a compiler, e.g. CC=clang CXX=clang++ make cpp

# Benchmarks (bench/README.md). BENCH_HOST_ID names the machine class and selects the baseline file.
BENCH_HOST_ID ?= dev
BENCH_OUT     ?= bench/results/$(BENCH_HOST_ID).json
BENCH_ARGS    ?=
# Instruction-count benchmarks (deterministic, valgrind --tool=callgrind; tools/bench_icount.py).
# The host id is a machine *class* (architecture), not a machine: counts do not depend on load.
BENCH_ICOUNT_HOST_ID ?= icount-x86_64
BENCH_ICOUNT_OUT     ?= bench/results/$(BENCH_ICOUNT_HOST_ID).json
BENCH_ICOUNT_BIN     ?= bench/target/release/ritsu-bench
BENCH_ICOUNT_ARGS    ?= --repeat 2
BENCH_ICOUNT_COMPARE_ARGS ?=

# DBT differential tests (tests/dbt-ref/README.md, tools/dbt_ref.sh). The checked-in golden files are
# compared with T0 on every `make test`; recording needs clang + lld and qemu-user (or an arm64 host).
DBT_REF_MANIFEST ?= tests/dbt-ref/Cargo.toml
DBT_REF_DIR      ?= build/dbt-ref
DBT_REF_GOLDEN   ?= tests/dbt-ref/golden/m0.golden
DBT_REF_NEW      ?= $(DBT_REF_DIR)/record.golden

.PHONY: help fmt fmt-check lint lint-aux test-rust test-aux fuzz-replay bench-test bench bench-compare bench-icount bench-icount-compare bench-check dbt-ref-gen dbt-ref-record dbt-ref-update dbt-ref-check dbt-ref-compare dbt-ref-verify-new dbt-ref-run dbt-ref-verify dbt-ref-fresh dbt-ref-smoke cpp cpp-fmt-check fuzz-smoke fuzz-smoke-rust fuzz-smoke-cpp graph links status test check clean

help:
	@echo "Ritsu make targets:"
	@echo "  help           this list (default)"
	@echo "  fmt            format Rust (cargo fmt) and C++ (clang-format, if installed)"
	@echo "  fmt-check      check Rust formatting (cargo fmt --check)"
	@echo "  lint           cargo clippy --workspace --all-targets -- -D warnings"
	@echo "  lint-aux       fmt --check + clippy -D warnings for the stand-alone packages fuzz/, bench/ and tests/dbt-ref/"
	@echo "  test-rust      cargo test --workspace"
	@echo "  test-aux       fuzz-replay + bench-test + dbt-ref-check"
	@echo "  fuzz-replay    replay fuzz/regressions and the seed corpus through the fuzz harnesses (stable, no cargo-fuzz)"
	@echo "  bench-test     unit tests of the benchmark workloads (hand-derived expected results)"
	@echo "  bench          run the benchmark suite (release) and write BENCH_OUT=bench/results/<BENCH_HOST_ID>.json"
	@echo "  bench-compare  bench + compare against bench/baselines/<BENCH_HOST_ID>.json (tools/bench_compare.py)"
	@echo "  bench-icount   deterministic host-instruction counts per workload (callgrind, needs valgrind) -> BENCH_ICOUNT_OUT"
	@echo "  bench-icount-compare  bench-icount + gate against bench/baselines/<BENCH_ICOUNT_HOST_ID>.json (2 %, tools/bench_compare.py)"
	@echo "  bench-check    quick smoke run of the bench pipeline + self-tests of bench_compare.py and bench_icount.py (part of check)"
	@echo "  dbt-ref-gen    generate the DBT test vectors (DBT_REF_SEED, DBT_REF_UNITS) -> build/dbt-ref/vectors.bin"
	@echo "  dbt-ref-record run the vectors on the reference (arm64 hardware, else qemu-aarch64) -> build/dbt-ref/record.golden"
	@echo "  dbt-ref-update install the recording as the checked-in golden file (DBT_REF_NEW, DBT_REF_GOLDEN; never replaces hardware by qemu)"
	@echo "  dbt-ref-check  T0 vs the checked-in golden files (cargo test of tests/dbt-ref; part of test)"
	@echo "  dbt-ref-compare  new recording vs checked-in golden file (CI job dbt-reference on arm64; informational while the golden file is qemu)"
	@echo "  dbt-ref-verify-new  T0 vs the new recording (CI job dbt-reference on arm64: T0 against real hardware)"
	@echo "  dbt-ref-run / dbt-ref-verify  nightly split: run vectors on the reference (arm64) / compare vectors.bin + results.raw with T0 (x86)"
	@echo "  dbt-ref-fresh  fresh seed (RITSU_TEST_SEED) on the live reference, compared with T0; no golden file involved"
	@echo "  dbt-ref-smoke  3.5k vectors on the live reference vs T0, a few seconds (part of check; skips without clang/qemu)"
	@echo "  cpp            configure, build and ctest compiler/air2spirv (Ninja, build/air2spirv)"
	@echo "  cpp-fmt-check  clang-format --dry-run --Werror on compiler/ C++ sources"
	@echo "  fuzz-smoke     60 s per fuzz target, Rust (cargo-fuzz) + C++ (libFuzzer); FUZZ_TIME=<s>; skips if tools are missing"
	@echo "  fuzz-smoke-rust / fuzz-smoke-cpp   the two halves of fuzz-smoke"
	@echo "  graph          check the crate dependency rule (tools/check_crate_graph.py)"
	@echo "  links          check relative links in *.md files (tools/check_md_links.py)"
	@echo "  status         validate status/*.json behind the live roadmap page"
	@echo "  test           test-rust + test-aux + cpp"
	@echo "  check          fmt-check + lint + lint-aux + test + bench-check + dbt-ref-smoke + cpp-fmt-check + graph + links + status (must be green before commit)"
	@echo "  clean          cargo clean and remove build/"

.DEFAULT_GOAL := help

fmt:
	$(CARGO) fmt --all
	@if command -v $(CLANG_FORMAT) >/dev/null 2>&1; then \
		files=`find compiler -type f \( -name "*.h" -o -name "*.hpp" -o -name "*.c" -o -name "*.cc" -o -name "*.cpp" \) -not -path "*/build/*" 2>/dev/null`; \
		if [ -n "$$files" ]; then echo "$$files" | xargs $(CLANG_FORMAT) -i; fi; \
	else \
		echo "note: $(CLANG_FORMAT) not installed, C++ formatting skipped"; \
	fi

fmt-check:
	$(CARGO) fmt --all -- --check

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# fuzz/ and bench/ are stand-alone packages (own [workspace]), so the workspace commands above
# do not see them. Both build on stable.
lint-aux:
	$(CARGO) fmt --manifest-path fuzz/Cargo.toml -- --check
	$(CARGO) clippy --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings
	$(CARGO) fmt --manifest-path bench/Cargo.toml -- --check
	$(CARGO) clippy --manifest-path bench/Cargo.toml --all-targets -- -D warnings
	$(CARGO) fmt --manifest-path $(DBT_REF_MANIFEST) -- --check
	$(CARGO) clippy --manifest-path $(DBT_REF_MANIFEST) --all-targets -- -D warnings

test-rust:
	$(CARGO) test --workspace

test-aux: fuzz-replay bench-test dbt-ref-check

# Checked-in fuzz findings (fuzz/regressions/) and the seed corpus run as ordinary tests (WP-QA-08).
fuzz-replay:
	$(CARGO) test --manifest-path fuzz/Cargo.toml

bench-test:
	$(CARGO) test --manifest-path bench/Cargo.toml

# --- DBT differential tests (WP-DBT-04) -----------------------------------------------------------
dbt-ref-gen:
	sh tools/dbt_ref.sh gen

dbt-ref-record:
	sh tools/dbt_ref.sh record

# Installs a recording as the golden file. A hardware recording is never replaced by a qemu one.
dbt-ref-update:
	$(CARGO) run --release --manifest-path $(DBT_REF_MANIFEST) -- promote --new $(DBT_REF_NEW) --into $(DBT_REF_GOLDEN) $(DBT_REF_PROMOTE_ARGS)

dbt-ref-check:
	@if ls tests/dbt-ref/golden/*.golden >/dev/null 2>&1; then \
		$(CARGO) test --manifest-path $(DBT_REF_MANIFEST); \
	else \
		echo "skip: no golden files in tests/dbt-ref/golden/ (record one: make dbt-ref-record dbt-ref-update)"; \
	fi

# Used by the arm64 CI job: $(DBT_REF_NEW) was just recorded on real hardware.
dbt-ref-compare:
	$(CARGO) run --release --manifest-path $(DBT_REF_MANIFEST) -- golden-diff $(DBT_REF_GOLDEN) $(DBT_REF_NEW)

dbt-ref-verify-new:
	$(CARGO) run --release --manifest-path $(DBT_REF_MANIFEST) -- check --golden $(DBT_REF_NEW)

dbt-ref-run:
	sh tools/dbt_ref.sh run

dbt-ref-verify:
	sh tools/dbt_ref.sh verify

dbt-ref-fresh:
	sh tools/dbt_ref.sh fresh

dbt-ref-smoke:
	sh tools/dbt_ref.sh smoke

cpp:
	@if [ -f $(CPP_SRC_DIR)/CMakeLists.txt ]; then \
		set -e; \
		$(CMAKE) -S $(CPP_SRC_DIR) -B $(CPP_BUILD_DIR) -G Ninja -DCMAKE_BUILD_TYPE=$(CPP_BUILD_TYPE) $(CPP_CMAKE_ARGS); \
		$(CMAKE) --build $(CPP_BUILD_DIR); \
		$(CTEST) --test-dir $(CPP_BUILD_DIR) --output-on-failure; \
	else \
		echo "skip: $(CPP_SRC_DIR)/CMakeLists.txt does not exist yet (C++ project not created)"; \
	fi

cpp-fmt-check:
	@if ! command -v $(CLANG_FORMAT) >/dev/null 2>&1; then \
		echo "warning: $(CLANG_FORMAT) not installed, C++ format check skipped"; \
	else \
		files=`find compiler -type f \( -name "*.h" -o -name "*.hpp" -o -name "*.c" -o -name "*.cc" -o -name "*.cpp" \) -not -path "*/build/*" 2>/dev/null`; \
		if [ -z "$$files" ]; then \
			echo "skip: no C++ sources under compiler/"; \
		else \
			echo "$$files" | xargs $(CLANG_FORMAT) --dry-run --Werror; \
		fi; \
	fi

# Not part of `check` (needs nightly + cargo-fuzz / clang and takes minutes); CI runs it on every PR.
# FUZZ_TIME=<seconds> per target, e.g. `make fuzz-smoke FUZZ_TIME=10`; FUZZ_TARGETS=<names> selects
# Rust targets. Each part skips cleanly when its tools are missing (FUZZ_REQUIRED=1: fail instead).
fuzz-smoke: fuzz-smoke-rust fuzz-smoke-cpp

fuzz-smoke-rust:
	sh fuzz/tools/smoke.sh $(FUZZ_TARGETS)

fuzz-smoke-cpp:
	sh fuzz/tools/smoke_cpp.sh

# Not part of `check` (minutes of wall-clock time, results depend on the machine).
bench:
	$(CARGO) run --release --manifest-path bench/Cargo.toml -- --host-id $(BENCH_HOST_ID) --output $(BENCH_OUT) $(BENCH_ARGS)

bench-compare: bench
	$(PYTHON) tools/bench_compare.py bench/baselines/$(BENCH_HOST_ID).json $(BENCH_OUT)

# Instruction counts: no timing involved, so the result is the same on a loaded cloud runner and
# the numbers may gate CI (bench/README.md, "Befehlszahlen"). Takes about 20 s plus the build.
bench-icount:
	$(CARGO) build --release --manifest-path bench/Cargo.toml
	$(PYTHON) tools/bench_icount.py --bin $(BENCH_ICOUNT_BIN) --host-id $(BENCH_ICOUNT_HOST_ID) --output $(BENCH_ICOUNT_OUT) $(BENCH_ICOUNT_ARGS)

bench-icount-compare: bench-icount
	$(PYTHON) tools/bench_compare.py $(BENCH_ICOUNT_COMPARE_ARGS) bench/baselines/$(BENCH_ICOUNT_HOST_ID).json $(BENCH_ICOUNT_OUT)

# Pipeline smoke test without meaningful numbers: debug build, --quick, schema validation.
bench-check:
	$(PYTHON) tools/bench_compare.py --self-test
	$(PYTHON) tools/bench_icount.py --self-test
	$(CARGO) run --manifest-path bench/Cargo.toml -- --allow-debug --icount-run bench_virtq_pop_push_1desc --reps 1
	$(CARGO) run --manifest-path bench/Cargo.toml -- --allow-debug --quick --filter bench_virtq --output build/bench-smoke.json
	$(PYTHON) tools/bench_compare.py --validate build/bench-smoke.json bench/baselines/*.json

graph:
	$(PYTHON) tools/check_crate_graph.py

links:
	$(PYTHON) tools/check_md_links.py

status:
	$(PYTHON) tools/check_status.py

test: test-rust test-aux cpp

# Run sequentially (also under make -j) so the first failure is the one reported.
check:
	$(MAKE) fmt-check
	$(MAKE) lint
	$(MAKE) lint-aux
	$(MAKE) test
	$(MAKE) bench-check
	$(MAKE) dbt-ref-smoke
	$(MAKE) cpp-fmt-check
	$(MAKE) graph
	$(MAKE) links
	$(MAKE) status

clean:
	$(CARGO) clean
	rm -rf build
