from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path
from typing import TYPE_CHECKING

from style_scan import Finding

if TYPE_CHECKING:
    from collections.abc import Callable

LINT_DIR = ".github/lint"
TEMPLATE_FILE_DEPTH = 3
MERGE_PROFILE_DEPTH = 4
ROOT_ONLY = frozenset(
    {
        ".editorconfig",
        ".prettierignore",
        ".shellcheckrc",
        "Directory.Build.props",
        "Directory.Build.targets",
        "clippy.toml",
        "pyrightconfig.json",
        "ruff.toml",
        "rustfmt.toml",
    }
)
ROOT_ONLY_PATTERN = re.compile(r"^(?:eslint|prettier|stylelint)\.config\.[cm]?[jt]s$")
LINT_ONLY = frozenset({"PSScriptAnalyzerSettings.psd1"})
LINT_FILES = LINT_ONLY | frozenset(
    {
        "CSharpStyle.cs",
        "Invoke-PSLint.ps1",
        "PSStyleRules.psm1",
        "pystyle.py",
        "rust-style.awk",
        "shell-files.awk",
        "shell-style.awk",
        "style_check.py",
        "style_configs.py",
        "style_scan.py",
        "style_sources.py",
        "tsconfig.base.json",
        "vendored-paths.local.txt",
        "vendored-paths.txt",
    }
)
OLD_LINT_DIR = ".lint/"
OLD_UNITY_LINT = re.compile(r"^lint/[^/]+\.(?:csproj|props|targets)$")
UNITY_PROJECT_DIRS = frozenset({"headless", f"{LINT_DIR}/unity"})
UNITY_BUILD_FILES = frozenset({"Directory.Build.props", "Directory.Build.targets"})
NEVER_NAMES = (
    r"\.eslintrc.*",
    r"\.eslintignore",
    r"\.prettierrc.*",
    r"\.stylelintrc.*",
    r"\.stylelintignore",
    r"\.ruff\.toml",
    r"\.rustfmt\.toml",
    r"\.clippy\.toml",
    r"\.globalconfig",
    r"GlobalSuppressions\.cs",
    r".*\.ruleset",
)
NEVER = re.compile("^(?:" + "|".join(NEVER_NAMES) + ")$")
PROJECT_FILE = re.compile(r"\.(?:csproj|props|targets)$")
PROJECT_DOWNGRADES = (
    r"<(?:NoWarn|WarningsNotAsErrors|CodeAnalysisRuleSet)\b",
    r"<Analysis(?:Level|Mode)\w*\b[^>]*>(?!\s*(?:latest-all|All)\s*<)",
    r"\bNoWarn\s*=",
    r"<(?:TreatWarningsAsErrors|EnforceCodeStyleInBuild|RunAnalyzers\w*|GenerateDocumentationFile)>\s*false",
    r"<Nullable>\s*(?:disable|warnings|annotations)",
    r"<WarningLevel>\s*[0-3]\s*<",
    r"<Features>[^<]*\brun-nullable-analysis\s*=\s*never",
    r"<(?:EditorConfigFiles|GlobalAnalyzerConfigFiles)\s+(?:Include|Remove|Update)\b",
)
PROJECT_DOWNGRADE = re.compile("|".join(PROJECT_DOWNGRADES), re.IGNORECASE)


def read_toml(path: Path) -> dict[str, object]:
    return tomllib.loads(path.read_text(encoding="utf-8-sig"))


def read_json(path: Path) -> object:
    loaded: object = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(loaded, dict):
        message = f"{path}: expected a JSON object"
        raise TypeError(message)
    return loaded


def has_key(value: object, *keys: str) -> bool:
    return isinstance(value, dict) and any(key in value for key in keys)


def pyproject_problem(path: Path, _root: Path) -> str | None:
    if has_key(read_toml(path).get("tool"), "ruff", "pyright"):
        return "[tool.ruff] or [tool.pyright] overrides the baseline (GEN-5)"
    return None


def package_problem(path: Path, _root: Path) -> str | None:
    if has_key(read_json(path), "eslintConfig", "prettier", "stylelint"):
        return "eslintConfig, prettier or stylelint key overrides the baseline (GEN-5)"
    return None


def workspace_has_lints(folder: Path, root: Path) -> bool:
    while True:
        manifest = folder / "Cargo.toml"
        workspace = read_toml(manifest).get("workspace") if manifest.is_file() else None
        if isinstance(workspace, dict):
            return "lints" in workspace
        if folder in {root, folder.parent}:
            return False
        folder = folder.parent


def cargo_problem(path: Path, root: Path) -> str | None:
    manifest = read_toml(path)
    lints = manifest.get("lints")
    if lints == {"workspace": True}:
        return None
    if lints is not None and path.parent != root:
        return "[lints] other than workspace = true overrides the baseline (GEN-5)"
    if "package" in manifest and workspace_has_lints(path.parent, root):
        return "[package] lacks [lints] workspace = true, so workspace lints don't apply (GEN-5)"
    return None


CONTENT_CHECKS: dict[str, Callable[[Path, Path], str | None]] = {
    "pyproject.toml": pyproject_problem,
    "package.json": package_problem,
    "Cargo.toml": cargo_problem,
}


def check_project(path: str, root: Path) -> list[Finding]:
    text = (root / path).read_text(encoding="utf-8-sig", errors="replace")
    return [
        Finding(path, text.count("\n", 0, match.start()) + 1, "project setting loosens the baseline (GEN-4)")
        for match in PROJECT_DOWNGRADE.finditer(text)
    ]


def beside_unity_projects(path: str, root: Path) -> bool:
    folder, _, name = path.rpartition("/")
    return folder in UNITY_PROJECT_DIRS and name in UNITY_BUILD_FILES and not (root / name).exists()


def misplaced(path: str, root: Path) -> str | None:
    folder, _, name = path.rpartition("/")
    if (not folder and name in LINT_FILES) or path.startswith(OLD_LINT_DIR):
        return f"belongs in {LINT_DIR}/ (GEN-5)"
    if OLD_UNITY_LINT.match(path):
        return f"Unity lint projects belong in {LINT_DIR}/unity/ (GEN-5)"
    if NEVER.match(name):
        return "lint or format config the baseline doesn't use (GEN-5)"
    is_root_only = name in ROOT_ONLY or ROOT_ONLY_PATTERN.match(name) is not None
    if folder and is_root_only and not beside_unity_projects(path, root):
        return "nested lint or format config overrides the baseline (GEN-5)"
    if folder not in {"", LINT_DIR} and name in LINT_ONLY:
        return "nested lint or format config overrides the baseline (GEN-5)"
    return None


def config_root(path: str, root: Path) -> Path:
    parts = path.split("/")
    distribution = root / "lint"
    source = distribution / "common" / LINT_DIR / Path(__file__).name
    if (
        len(parts) < TEMPLATE_FILE_DEPTH
        or parts[0] != "lint"
        or not (distribution / "Install-Lint.ps1").is_file()
        or not source.is_file()
        or source.read_bytes() != Path(__file__).read_bytes()
    ):
        return root
    template = distribution / parts[1]
    manifest = template / "install.psd1"
    if len(parts) > MERGE_PROFILE_DEPTH and parts[2] == "merge" and manifest.is_file():
        uses = re.findall(r"\bUse\s*=\s*'([^']+)'", manifest.read_text(encoding="utf-8-sig"))
        if any(Path(use).parent.as_posix() == "/".join(parts[2:-1]) for use in uses):
            return root.joinpath(*parts[:-1])
    return template


def check_config(path: str, root: Path) -> list[Finding]:
    try:
        context = config_root(path, root)
        relative = (root / path).relative_to(context).as_posix()
        return [Finding(path, finding.line, finding.message) for finding in check_effective_config(relative, context)]
    except (OSError, ValueError, TypeError) as error:
        return [Finding(path, 0, f"can't read config: {error}. Fix the file and run the check again (GEN-19)")]


def check_effective_config(path: str, root: Path) -> list[Finding]:
    name = path.rsplit("/", 1)[-1]
    problem = misplaced(path, root)
    if problem:
        return [Finding(path, 0, problem)]
    if PROJECT_FILE.search(name):
        return check_project(path, root)
    check = CONTENT_CHECKS.get(name)
    message = check(root / path, root) if check else None
    return [Finding(path, 0, message)] if message else []
