#!/usr/bin/env python3
"""Check that relative links in the repository's Markdown files resolve.

Checks inline links/images `[text](target)` and reference definitions
`[id]: target` in every *.md file (excluding target/, build/, legacy/, .git/).
http(s)/mailto/other URL schemes and pure `#anchor` links are ignored; a
`#anchor` suffix is stripped (anchors themselves are not verified). Links
inside fenced code blocks and inline code spans are ignored.

Usage:
    python3 tools/check_md_links.py            # check the repo
    python3 tools/check_md_links.py --self-test

Exit status: 0 = all links ok, 1 = broken links, 2 = usage error. Stdlib only.
"""

from __future__ import annotations

import os
import re
import sys
import tempfile
from pathlib import Path
from urllib.parse import unquote

EXCLUDED_DIRS = {"target", "build", "legacy", ".git", "node_modules"}

INLINE_RE = re.compile(r"!?\[(?:[^\[\]]|\[[^\]]*\])*\]\(\s*(<[^>]*>|[^)\s]*(?:\([^)\s]*\)[^)\s]*)*)(?:\s+(?:\"[^\"]*\"|'[^']*'|\([^)]*\)))?\s*\)")
REFDEF_RE = re.compile(r"^ {0,3}\[[^\]]+\]:\s*(<[^>]*>|\S+)")
FENCE_RE = re.compile(r"^ {0,3}(`{3,}|~{3,})")
CODESPAN_RE = re.compile(r"(`+)(?:(?!\1).)+?\1")
SCHEME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9+.\-]*:")


def iter_md_files(root: Path):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in EXCLUDED_DIRS)
        for fn in sorted(filenames):
            if fn.endswith(".md"):
                yield Path(dirpath) / fn


def extract_links(text: str):
    """Yield (line_number, raw_target) for each link outside code."""
    fence: str | None = None
    for lineno, line in enumerate(text.splitlines(), start=1):
        m = FENCE_RE.match(line)
        if m:
            marker = m.group(1)
            if fence is None:
                fence = marker[0] * 3
            elif marker[0] == fence[0] and len(marker) >= 3:
                fence = None
            continue
        if fence is not None:
            continue
        stripped = CODESPAN_RE.sub(lambda mm: " " * len(mm.group(0)), line)
        for mm in INLINE_RE.finditer(stripped):
            yield lineno, mm.group(1)
        rd = REFDEF_RE.match(stripped)
        if rd:
            yield lineno, rd.group(1)


def resolve(md_file: Path, root: Path, raw: str) -> Path | None:
    """Return the path a link must point to, or None if it is to be ignored."""
    target = raw.strip()
    if target.startswith("<") and target.endswith(">"):
        target = target[1:-1]
    if not target or target.startswith("#") or target.startswith("//"):
        return None
    if SCHEME_RE.match(target):  # http:, https:, mailto:, ftp:, ...
        return None
    target = target.split("#", 1)[0].split("?", 1)[0]
    if not target:
        return None
    target = unquote(target)
    if target.startswith("/"):
        return root / target.lstrip("/")
    return md_file.parent / target


def check(root: Path) -> list[str]:
    broken: list[str] = []
    for md in iter_md_files(root):
        try:
            text = md.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as exc:
            broken.append(f"{md.relative_to(root)}: cannot read file: {exc}")
            continue
        for lineno, raw in extract_links(text):
            dest = resolve(md, root, raw)
            if dest is None:
                continue
            if not os.path.lexists(os.path.normpath(dest)):
                broken.append(f"{md.relative_to(root)}:{lineno}: broken link '{raw}'")
    return broken


def self_test() -> int:
    failures: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "docs").mkdir()
        (root / "legacy").mkdir()
        (root / "docs" / "exists.md").write_text("# x\n")
        (root / "legacy" / "bad.md").write_text("[x](nope.md)\n")
        (root / "README.md").write_text(
            "[ok](docs/exists.md) [anchor](docs/exists.md#sec) [web](https://example.com/a)\n"
            "[mail](mailto:a@b.c) [self](#top) [dir](docs/) ![img](docs/exists.md)\n"
            "[title](docs/exists.md \"Title\") [enc](docs/exists.md)\n"
            "`[code](missing-in-code.md)`\n"
            "```\n[fenced](missing-in-fence.md)\n```\n"
            "[ref]: docs/exists.md\n"
            "[bad1](docs/missing.md) and [bad2](docs/missing2.md#a)\n"
            "[ref2]: docs/missing-ref.md\n"
        )
        got = sorted(b.split("broken link ")[1] for b in check(root))
        want = sorted(["'docs/missing.md'", "'docs/missing2.md#a'", "'docs/missing-ref.md'"])
        if got != want:
            failures.append(f"expected {want}, got {got}")
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
    broken = check(root)
    count = sum(1 for _ in iter_md_files(root))
    if broken:
        print(f"{len(broken)} broken link(s) in {count} markdown file(s):", file=sys.stderr)
        for b in broken:
            print(f"  {b}", file=sys.stderr)
        return 1
    print(f"links ok: {count} markdown file(s) checked")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
