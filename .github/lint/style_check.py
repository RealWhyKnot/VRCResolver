#!/usr/bin/env python3
from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

from style_configs import LINT_DIR, check_config
from style_scan import Finding
from style_sources import check_generated, check_source

VENDORED_FILE = "vendored-paths.txt"
LOCAL_FILE = "vendored-paths.local.txt"
SEGMENT = r"(?!\.+/)[A-Za-z0-9_.@+-]+/"
ENTRY = re.compile(rf"/(?:{SEGMENT})+|{SEGMENT}")


@dataclass(frozen=True, slots=True)
class VendoredPaths:
    names: frozenset[str]
    prefixes: tuple[str, ...]

    def covers(self, path: str) -> bool:
        if path.startswith(self.prefixes):
            return True
        return any(part in self.names for part in path.split("/")[:-1])


def read_entries(path: Path) -> list[str]:
    entries: list[str] = []
    for number, raw in enumerate(path.read_text(encoding="utf-8-sig").splitlines(), 1):
        entry = raw.strip()
        if not entry:
            continue
        if not ENTRY.fullmatch(entry):
            message = f"{path.name}:{number}: {entry!r} isn't a folder name/ or a /path/ from the repo root"
            raise ValueError(message)
        entries.append(entry)
    return entries


def vendored_paths(entries: list[str]) -> VendoredPaths:
    names = {entry.strip("/") for entry in entries if not entry.startswith("/")}
    prefixes = tuple(entry.lstrip("/") for entry in entries if entry.startswith("/"))
    return VendoredPaths(frozenset(names), prefixes)


def unfolded(shared: list[str], local: list[str]) -> list[Finding]:
    return [
        Finding(f"{LINT_DIR}/{LOCAL_FILE}", 0, f"{entry} isn't in {VENDORED_FILE} yet. Run Install-Lint again (GEN-1)")
        for entry in local
        if entry not in shared
    ]


def tracked_files(root: Path) -> list[str]:
    git = shutil.which("git")
    if git is None:
        message = "git isn't on PATH"
        raise OSError(message)
    result = subprocess.run(
        [git, "-C", str(root), "ls-files", "-z"],
        capture_output=True,
        check=True,
        timeout=120,
    )
    return [entry for entry in result.stdout.decode("utf-8", errors="replace").split("\0") if entry]


def check_repo(root: Path, vendored: VendoredPaths) -> list[Finding]:
    own = [path for path in tracked_files(root) if not vendored.covers(path)]
    tracked = frozenset(own)
    findings: list[Finding] = []
    for path in own:
        full = root / path
        if not full.is_file():
            continue
        findings.extend(check_config(path, root))
        findings.extend(check_generated(path, tracked))
        try:
            text = full.read_text(encoding="utf-8-sig", errors="replace")
        except OSError as error:
            findings.append(Finding(path, 0, f"can't read: {error.strerror}"))
            continue
        findings.extend(check_source(path, text))
    return sorted(findings)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check a repo for comments, lint suppressions, long files and nested lint configs.",
    )
    parser.add_argument("root", nargs="?", default=".", help="repo root (default: the current folder)")
    args = parser.parse_args()
    root = Path(str(args.root)).resolve()
    list_path = Path(__file__).resolve().with_name(VENDORED_FILE)
    if not list_path.is_file():
        sys.stderr.write(f"{VENDORED_FILE} is missing next to {Path(__file__).name}. Run Install-Lint again.\n")
        return 2
    lint = root / LINT_DIR
    local_path = lint / LOCAL_FILE
    try:
        shared = read_entries(list_path)
        local = read_entries(local_path) if local_path.is_file() else []
    except (OSError, ValueError) as error:
        sys.stderr.write(f"can't read the vendored paths: {error}\n")
        return 2
    try:
        findings = check_repo(root, vendored_paths(shared + local))
    except (subprocess.CalledProcessError, OSError) as error:
        sys.stderr.write(f"can't list the files in {root} with git: {error}\n")
        return 2
    if list_path.parent == lint:
        findings = sorted(findings + unfolded(shared, local))
    for finding in findings:
        sys.stdout.write(finding.render() + "\n")
    if findings:
        sys.stdout.write(f"{len(findings)} findings\n")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
