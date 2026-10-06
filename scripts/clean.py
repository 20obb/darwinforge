#!/usr/bin/env python3

import argparse
import shutil
import sys
from pathlib import Path

DIRECTORY_NAMES = {
    ".cache",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "__pycache__",
    "build",
    "dist",
    "target",
}

FILE_SUFFIXES = (
    ".exe",
    ".ilk",
    ".ipa",
    ".log",
    ".pdb",
    ".pyc",
    ".pyo",
)

FIXED_FILE_NAMES = {
    ".DS_Store",
    "Thumbs.db",
}


def should_skip(path: Path, root: Path) -> bool:
    try:
        relative = path.relative_to(root)
    except ValueError:
        return True
    return any(part in {".git", ".hg", ".svn"} for part in relative.parts)


def candidates(root: Path) -> list[Path]:
    found: list[Path] = []
    for path in root.rglob("*"):
        if should_skip(path, root):
            continue
        if path.is_symlink():
            if path.name in DIRECTORY_NAMES or path.suffix in FILE_SUFFIXES:
                found.append(path)
            continue
        if path.is_dir() and path.name in DIRECTORY_NAMES:
            found.append(path)
            continue
        if path.is_file() and (path.name in FIXED_FILE_NAMES or path.suffix in FILE_SUFFIXES):
            found.append(path)
    found.sort(key=lambda item: (len(item.parts), str(item)))
    directory_targets = {path for path in found if path.is_dir()}
    return [
        path
        for path in found
        if not any(parent in directory_targets for parent in path.parents)
    ]


def remove(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink(missing_ok=True)
    elif path.is_dir():
        shutil.rmtree(path, ignore_errors=False)


def main() -> int:
    parser = argparse.ArgumentParser(description="Remove generated build artefacts before uploading the repository.")
    parser.add_argument("--root", type=Path, default=Path.cwd(), help="Repository root to clean.")
    parser.add_argument("--dry-run", action="store_true", help="List files without deleting them.")
    parser.add_argument("-y", "--yes", action="store_true", help="Do not ask for confirmation.")
    args = parser.parse_args()

    root = args.root.resolve()
    if not root.is_dir():
        print(f"error: {root} is not a directory", file=sys.stderr)
        return 2

    targets = candidates(root)
    if not targets:
        print(f"nothing to clean under {root}")
        return 0

    for target in targets:
        print(target)

    if args.dry_run:
        print(f"{len(targets)} item(s) would be removed")
        return 0

    if not args.yes:
        if not sys.stdin.isatty():
            print("error: no terminal available; re-run with --yes", file=sys.stderr)
            return 5
        answer = input(f"Remove {len(targets)} generated item(s)? [y/N] ").strip().lower()
        if answer not in {"y", "yes"}:
            print("nothing removed")
            return 0

    failures = 0
    for target in targets:
        try:
            if target.exists() or target.is_symlink():
                remove(target)
                print(f"removed {target}")
        except OSError as error:
            failures += 1
            print(f"error: cannot remove {target}: {error}", file=sys.stderr)

    if failures:
        return 1
    print(f"removed {len(targets)} item(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
