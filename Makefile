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
# Set CC/CXX in the environment to pick a compiler, e.g. CC=clang CXX=clang++ make cpp

.PHONY: help fmt fmt-check lint test-rust cpp cpp-fmt-check graph links status test check clean

help:
	@echo "Ritsu make targets:"
	@echo "  help           this list (default)"
	@echo "  fmt            format Rust (cargo fmt) and C++ (clang-format, if installed)"
	@echo "  fmt-check      check Rust formatting (cargo fmt --check)"
	@echo "  lint           cargo clippy --workspace --all-targets -- -D warnings"
	@echo "  test-rust      cargo test --workspace"
	@echo "  cpp            configure, build and ctest compiler/air2spirv (Ninja, build/air2spirv)"
	@echo "  cpp-fmt-check  clang-format --dry-run --Werror on compiler/ C++ sources"
	@echo "  graph          check the crate dependency rule (tools/check_crate_graph.py)"
	@echo "  links          check relative links in *.md files (tools/check_md_links.py)"
	@echo "  status         validate status/*.json behind the live roadmap page"
	@echo "  test           test-rust + cpp"
	@echo "  check          fmt-check + lint + test + graph + links + status (must be green before commit)"
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

test-rust:
	$(CARGO) test --workspace

cpp:
	@if [ -f $(CPP_SRC_DIR)/CMakeLists.txt ]; then \
		set -e; \
		$(CMAKE) -S $(CPP_SRC_DIR) -B $(CPP_BUILD_DIR) -G Ninja -DCMAKE_BUILD_TYPE=$(CPP_BUILD_TYPE); \
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

graph:
	$(PYTHON) tools/check_crate_graph.py

links:
	$(PYTHON) tools/check_md_links.py

status:
	$(PYTHON) tools/check_status.py

test: test-rust cpp

# Run sequentially (also under make -j) so the first failure is the one reported.
check:
	$(MAKE) fmt-check
	$(MAKE) lint
	$(MAKE) test
	$(MAKE) graph
	$(MAKE) links
	$(MAKE) status

clean:
	$(CARGO) clean
	rm -rf build
