#!/usr/bin/env python3
"""Verify the ritsu-* crate dependency rule (docs/ARCHITECTURE.md section 4).

Runs `cargo metadata --no-deps` and checks that every workspace crate named
`ritsu-*` depends only on the ritsu-* crates allowed for it in ALLOWED below.
Dev-dependencies and build-dependencies follow the same map (keeps the graph
acyclic and the crates independently testable).

Usage:
    python3 tools/check_crate_graph.py             # check the workspace
    python3 tools/check_crate_graph.py --self-test # run built-in unit checks

Exit status: 0 = ok, 1 = rule violation, 2 = tool/usage error.
Stdlib only.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Single source of truth for the allowed direct dependencies between ritsu-*
# crates. Mirrors docs/ARCHITECTURE.md section 4 ("Abhaengigkeitsregel").
# Adding a crate or an edge means: change this map AND ARCHITECTURE.md (and,
# for a new edge, think about an ADR first).
# ---------------------------------------------------------------------------
ALLOWED: dict[str, list[str]] = {
    "ritsu-core": [],
    "ritsu-mem": ["ritsu-core"],
    "ritsu-dbt": ["ritsu-core", "ritsu-mem"],
    "ritsu-virtio": ["ritsu-core", "ritsu-mem"],
    "ritsu-platform": ["ritsu-core", "ritsu-mem", "ritsu-dbt", "ritsu-virtio"],
    "ritsu-gpu-proto": ["ritsu-core"],
    "ritsu-gpu-host": ["ritsu-core", "ritsu-gpu-proto"],
    "ritsu-vmm": [
        "ritsu-core",
        "ritsu-mem",
        "ritsu-dbt",
        "ritsu-virtio",
        "ritsu-platform",
        "ritsu-gpu-proto",
        "ritsu-gpu-host",
    ],
}

PREFIX = "ritsu-"
KIND_NAMES = {None: "dependency", "dev": "dev-dependency", "build": "build-dependency"}


def validate(packages: list[dict], allowed: dict[str, list[str]] = ALLOWED) -> list[str]:
    """Return a list of human-readable violations (empty list = ok).

    `packages` is the `packages` array of `cargo metadata` output; only the
    fields `name` and `dependencies[].{name,kind}` are used.
    """
    errors: list[str] = []
    for pkg in sorted(packages, key=lambda p: p["name"]):
        name = pkg["name"]
        if not name.startswith(PREFIX):
            continue
        if name not in allowed:
            errors.append(
                f"{name}: unknown ritsu-* crate. Add it to ALLOWED in "
                f"tools/check_crate_graph.py and to docs/ARCHITECTURE.md section 4."
            )
            continue
        seen: set[tuple[str, str | None]] = set()
        for dep in pkg.get("dependencies", []):
            dep_name = dep["name"]
            if not dep_name.startswith(PREFIX):
                continue
            kind = dep.get("kind")
            if (dep_name, kind) in seen:  # same dep under several target cfgs
                continue
            seen.add((dep_name, kind))
            kind_name = KIND_NAMES.get(kind, str(kind))
            if dep_name == name:
                errors.append(f"{name}: depends on itself ({kind_name}).")
            elif dep_name not in allowed:
                errors.append(
                    f"{name}: {kind_name} on unknown ritsu-* crate '{dep_name}'. Add it to "
                    f"ALLOWED in tools/check_crate_graph.py and to docs/ARCHITECTURE.md section 4."
                )
            elif dep_name not in allowed[name]:
                permitted = ", ".join(allowed[name]) or "(none)"
                errors.append(
                    f"{name}: {kind_name} on '{dep_name}' is not allowed by the dependency rule "
                    f"(docs/ARCHITECTURE.md section 4). Allowed for {name}: {permitted}."
                )
    errors.extend(_find_cycles(packages, allowed))
    return errors


def _find_cycles(packages: list[dict], allowed: dict[str, list[str]]) -> list[str]:
    """Defensive cycle check on the actual graph (ritsu-* edges only)."""
    edges: dict[str, set[str]] = {}
    for pkg in packages:
        if pkg["name"].startswith(PREFIX):
            edges[pkg["name"]] = {
                d["name"]
                for d in pkg.get("dependencies", [])
                if d["name"].startswith(PREFIX) and d["name"] != pkg["name"]
            }
    errors: list[str] = []
    state: dict[str, int] = {}  # 1 = on stack, 2 = done

    def visit(node: str, path: list[str]) -> None:
        state[node] = 1
        for nxt in sorted(edges.get(node, ())):
            if state.get(nxt) == 1:
                cycle = path[path.index(nxt):] + [nxt] if nxt in path else [node, nxt]
                errors.append("dependency cycle: " + " -> ".join(cycle))
            elif nxt not in state:
                visit(nxt, path + [nxt])
        state[node] = 2

    for n in sorted(edges):
        if n not in state:
            visit(n, [n])
    return errors


def run_cargo_metadata() -> list[dict]:
    try:
        out = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except FileNotFoundError:
        print("error: `cargo` not found in PATH", file=sys.stderr)
        sys.exit(2)
    except subprocess.CalledProcessError as exc:
        print(f"error: `cargo metadata` failed:\n{exc.stderr}", file=sys.stderr)
        sys.exit(2)
    return json.loads(out)["packages"]


def _pkg(name: str, *deps: tuple[str, str | None]) -> dict:
    return {"name": name, "dependencies": [{"name": n, "kind": k} for n, k in deps]}


def self_test() -> int:
    failures: list[str] = []

    def expect(label: str, packages: list[dict], want_errors: int, needle: str = "") -> None:
        errs = validate(packages)
        ok = len(errs) == want_errors and (not needle or any(needle in e for e in errs))
        if not ok:
            failures.append(f"{label}: expected {want_errors} error(s) containing {needle!r}, got {errs}")

    expect("empty workspace", [], 0)
    expect("core without deps", [_pkg("ritsu-core")], 0)
    expect(
        "allowed edge + external dep",
        [_pkg("ritsu-dbt", ("ritsu-core", None), ("ritsu-mem", None), ("thiserror", None))],
        0,
    )
    expect("core -> mem forbidden", [_pkg("ritsu-core", ("ritsu-mem", None))], 1, "not allowed")
    expect("dbt -> platform forbidden", [_pkg("ritsu-dbt", ("ritsu-platform", None))], 1, "ritsu-platform")
    expect(
        "gpu-host must not see the CPU emulation",
        [_pkg("ritsu-gpu-host", ("ritsu-dbt", None))],
        1,
        "ritsu-dbt",
    )
    expect("dev-dep follows the map", [_pkg("ritsu-mem", ("ritsu-dbt", "dev"))], 1, "dev-dependency")
    expect("unknown crate", [_pkg("ritsu-newthing")], 1, "unknown ritsu-* crate")
    expect("dep on unknown crate", [_pkg("ritsu-vmm", ("ritsu-newthing", None))], 1, "newthing")
    expect("non-ritsu package ignored", [_pkg("serde", ("ritsu-core", None))], 0)
    expect(
        "cycle reported",
        [_pkg("ritsu-core", ("ritsu-mem", None)), _pkg("ritsu-mem", ("ritsu-core", None))],
        2,  # one forbidden edge (core->mem) + one cycle
        "cycle",
    )
    expect(
        "duplicate target-cfg entries counted once",
        [_pkg("ritsu-core", ("ritsu-mem", None), ("ritsu-mem", None))],
        1,
    )

    # The ALLOWED map itself must be closed and acyclic.
    for crate, deps in ALLOWED.items():
        for d in deps:
            if d not in ALLOWED:
                failures.append(f"ALLOWED[{crate}] mentions unknown crate {d}")
            if crate in ALLOWED.get(d, []):
                failures.append(f"ALLOWED has a 2-cycle between {crate} and {d}")
    synthetic = [_pkg(c, *[(d, None) for d in deps]) for c, deps in ALLOWED.items()]
    if validate(synthetic):
        failures.append(f"ALLOWED map violates itself: {validate(synthetic)}")

    if failures:
        print("self-test FAILED:")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("self-test ok")
    return 0


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    if any(a in ("-h", "--help") for a in argv):
        print(__doc__)
        return 0
    if argv:
        print(f"error: unknown argument(s): {' '.join(argv)}", file=sys.stderr)
        return 2
    root = Path(__file__).resolve().parent.parent
    try:
        os.chdir(root)
    except OSError as exc:  # pragma: no cover
        print(f"error: cannot chdir to {root}: {exc}", file=sys.stderr)
        return 2
    packages = run_cargo_metadata()
    errors = validate(packages)
    ritsu = sorted(p["name"] for p in packages if p["name"].startswith(PREFIX))
    missing = sorted(set(ALLOWED) - set(ritsu))
    if errors:
        print("crate dependency rule violated (docs/ARCHITECTURE.md section 4):", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        return 1
    print(f"crate graph ok: {len(ritsu)} ritsu-* crates checked")
    if missing:
        print(f"note: in ALLOWED but not in the workspace: {', '.join(missing)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
