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

.PHONY: help fmt fmt-check lint lint-aux test-rust test-aux fuzz-replay bench-test bench bench-compare bench-check cpp cpp-fmt-check fuzz-smoke fuzz-smoke-rust fuzz-smoke-cpp graph links status test check clean

help:
	@echo "Ritsu make targets:"
	@echo "  help           this list (default)"
	@echo "  fmt            format Rust (cargo fmt) and C++ (clang-format, if installed)"
	@echo "  fmt-check      check Rust formatting (cargo fmt --check)"
	@echo "  lint           cargo clippy --workspace --all-targets -- -D warnings"
	@echo "  lint-aux       fmt --check + clippy -D warnings for the stand-alone packages fuzz/ and bench/"
	@echo "  test-rust      cargo test --workspace"
	@echo "  test-aux       fuzz-replay + bench-test"
	@echo "  fuzz-replay    replay fuzz/regressions and the seed corpus through the fuzz harnesses (stable, no cargo-fuzz)"
	@echo "  bench-test     unit tests of the benchmark workloads (hand-derived expected results)"
	@echo "  bench          run the benchmark suite (release) and write BENCH_OUT=bench/results/<BENCH_HOST_ID>.json"
	@echo "  bench-compare  bench + compare against bench/baselines/<BENCH_HOST_ID>.json (tools/bench_compare.py)"
	@echo "  bench-check    quick smoke run of the bench pipeline + bench_compare.py --self-test (part of check)"
	@echo "  cpp            configure, build and ctest compiler/air2spirv (Ninja, build/air2spirv)"
	@echo "  cpp-fmt-check  clang-format --dry-run --Werror on compiler/ C++ sources"
	@echo "  fuzz-smoke     60 s per fuzz target, Rust (cargo-fuzz) + C++ (libFuzzer); FUZZ_TIME=<s>; skips if tools are missing"
	@echo "  fuzz-smoke-rust / fuzz-smoke-cpp   the two halves of fuzz-smoke"
	@echo "  graph          check the crate dependency rule (tools/check_crate_graph.py)"
	@echo "  links          check relative links in *.md files (tools/check_md_links.py)"
	@echo "  status         validate status/*.json behind the live roadmap page"
	@echo "  test           test-rust + test-aux + cpp"
	@echo "  check          fmt-check + lint + lint-aux + test + bench-check + cpp-fmt-check + graph + links + status (must be green before commit)"
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

test-rust:
	$(CARGO) test --workspace

test-aux: fuzz-replay bench-test

# Checked-in fuzz findings (fuzz/regressions/) and the seed corpus run as ordinary tests (WP-QA-08).
fuzz-replay:
	$(CARGO) test --manifest-path fuzz/Cargo.toml

bench-test:
	$(CARGO) test --manifest-path bench/Cargo.toml

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

# Pipeline smoke test without meaningful numbers: debug build, --quick, schema validation.
bench-check:
	$(PYTHON) tools/bench_compare.py --self-test
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
	$(MAKE) cpp-fmt-check
	$(MAKE) graph
	$(MAKE) links
	$(MAKE) status

clean:
	$(CARGO) clean
	rm -rf build
