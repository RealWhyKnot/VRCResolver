import ast
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

VENDORED_FILE = "vendored-paths.txt"
VENDORED_PLACES = (".", "../../../common/.github/lint")
MODULE_NAME = re.compile(r"[a-z_][a-z0-9_]*")
BUILD_SYSTEM = re.compile(r"^\[build-system\]", re.MULTILINE)
BROAD_EXCEPTIONS = frozenset({"Exception", "BaseException", "builtins.Exception", "builtins.BaseException"})
TEXT_FLAGS = ("text", "universal_newlines")
SUBPROCESS_WAITS = frozenset({"subprocess.run", "subprocess.call", "subprocess.check_call", "subprocess.check_output"})
SUBPROCESS_CALLS = SUBPROCESS_WAITS | {"subprocess.Popen"}
TIMEOUT_POSITIONS = {
    "urllib.request.urlopen": 2,
    "socket.create_connection": 1,
    "http.client.HTTPConnection": 2,
    "http.client.HTTPSConnection": 2,
}
HTTP_VERBS = ("get", "post", "put", "patch", "delete", "head", "options", "request")
NEW_CONNECTIONS = frozenset(
    {f"{library}.{verb}" for library in ("requests", "httpx") for verb in HTTP_VERBS}
    | {
        "urllib.request.urlopen",
        "requests.Session",
        "httpx.Client",
        "httpx.AsyncClient",
        "http.client.HTTPConnection",
        "http.client.HTTPSConnection",
        "aiohttp.ClientSession",
    },
)
BLE_WAITS = frozenset({"connect", "notified", "indicated", "written"})
PATCHERS = frozenset({"unittest.mock.patch", "mock.patch"})
STANDARD_MODULES = frozenset(sys.stdlib_module_names)
FUNCTIONS = (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)
LOOPS = (ast.While, ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)
TOP_LEVEL_DEFINITIONS = (
    ast.Import,
    ast.ImportFrom,
    ast.FunctionDef,
    ast.AsyncFunctionDef,
    ast.ClassDef,
    ast.Assign,
    ast.AnnAssign,
    ast.Raise,
    ast.Pass,
)


@dataclass(frozen=True, slots=True)
class Finding:
    path: Path
    line: int
    column: int
    rule: str
    message: str

    def render(self) -> str:
        return f"{display(self.path)}:{self.line}:{self.column}: {self.rule} {self.message}"


@dataclass(slots=True)
class Scope:
    loops: list[set[str]] = field(default_factory=list[set[str]])
    wall_clock_names: set[str] = field(default_factory=set[str])


@dataclass(frozen=True, slots=True)
class Listing:
    sources: list[Path]
    projects: list[Path]


@dataclass(slots=True, kw_only=True)
class FileCheck:
    path: Path
    aliases: dict[str, str]
    is_firmware: bool
    uses_aioble: bool
    findings: list[Finding] = field(default_factory=list[Finding])
    base_errors: list[ast.ClassDef] = field(default_factory=list[ast.ClassDef])

    def add(self, node: ast.stmt | ast.expr | ast.excepthandler, rule: str, message: str) -> None:
        self.add_at(node.lineno, node.col_offset, rule, message)

    def add_at(self, line: int, column: int, rule: str, message: str) -> None:
        self.findings.append(Finding(self.path, line, column + 1, rule, message))


def display(path: Path) -> str:
    try:
        return str(path.relative_to(Path.cwd()))
    except ValueError:
        return str(path)


def import_aliases(tree: ast.Module) -> dict[str, str]:
    aliases: dict[str, str] = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for name in node.names:
                top = name.name.partition(".")[0]
                aliases[name.asname or top] = name.name if name.asname else top
        elif isinstance(node, ast.ImportFrom) and node.module and not node.level:
            for name in node.names:
                aliases[name.asname or name.name] = f"{node.module}.{name.name}"
    return aliases


def qualified(node: ast.expr, aliases: dict[str, str]) -> str:
    if isinstance(node, ast.Name):
        return aliases.get(node.id, node.id)
    if isinstance(node, ast.Attribute):
        base = qualified(node.value, aliases)
        return f"{base}.{node.attr}" if base else ""
    return ""


def keyword_value(call: ast.Call, name: str) -> ast.expr | None:
    return next((item.value for item in call.keywords if item.arg == name), None)


def passes_kwargs(call: ast.Call) -> bool:
    return any(item.arg is None for item in call.keywords)


def has_keyword(call: ast.Call, name: str) -> bool:
    return keyword_value(call, name) is not None or passes_kwargs(call)


def is_true(node: ast.expr | None) -> bool:
    return isinstance(node, ast.Constant) and node.value is True


def opens_binary(call: ast.Call) -> bool:
    mode = keyword_value(call, "mode")
    if mode is None and len(call.args) > 1:
        mode = call.args[1]
    return isinstance(mode, ast.Constant) and isinstance(mode.value, str) and "b" in mode.value


def check_suppress(check: FileCheck, call: ast.Call, name: str) -> None:
    if name != "contextlib.suppress":
        return
    for argument in call.args:
        items = argument.elts if isinstance(argument, ast.Tuple) else [argument]
        if any(qualified(item, check.aliases) in BROAD_EXCEPTIONS for item in items):
            check.add(call, "PY-9", "suppress(Exception) hides every error; catch the exceptions the call can raise")


def catches_everything(check: FileCheck, handler: ast.ExceptHandler) -> bool:
    if handler.type is None:
        return False
    items = handler.type.elts if isinstance(handler.type, ast.Tuple) else [handler.type]
    return any(qualified(item, check.aliases) in BROAD_EXCEPTIONS for item in items)


def reports_error(node: ast.AST) -> bool:
    if isinstance(node, ast.Raise):
        return True
    if not isinstance(node, ast.Call):
        return False
    logs_traceback = isinstance(node.func, ast.Attribute) and node.func.attr == "exception"
    return logs_traceback or has_keyword(node, "exc_info")


def check_handler(check: FileCheck, handler: ast.ExceptHandler) -> None:
    if not catches_everything(check, handler):
        return
    last = handler.body[-1]
    if isinstance(last, ast.Raise) and last.exc is None:
        return
    if any(reports_error(node) for statement in handler.body for node in ast.walk(statement)):
        check.add(handler, "PY-9", "a catch-all that logs or raises still catches everything; catch what you handle")


def check_encoding(check: FileCheck, call: ast.Call, name: str) -> None:
    if has_keyword(call, "encoding"):
        return
    if name == "io.open" and not opens_binary(call):
        check.add(call, "PY-2", "io.open in text mode needs encoding=")
    elif name in SUBPROCESS_CALLS and any(is_true(keyword_value(call, flag)) for flag in TEXT_FLAGS):
        check.add(call, "PY-2", "subprocess with text=True needs encoding=")


def check_timeout(check: FileCheck, call: ast.Call, name: str) -> None:
    if has_keyword(call, "timeout"):
        return
    position = TIMEOUT_POSITIONS.get(name)
    if (position is not None and len(call.args) <= position) or name in SUBPROCESS_WAITS:
        check.add(call, "PY-4", f"{name} needs a timeout")


def check_patch(check: FileCheck, call: ast.Call, name: str) -> None:
    if name not in PATCHERS or not call.args:
        return
    target = call.args[0]
    if isinstance(target, ast.Constant) and isinstance(target.value, str):
        module = target.value.partition(".")[0]
        if module in STANDARD_MODULES:
            check.add(call, "PY-21", f"patch {target.value!r} through the module under test that looks it up")


def check_ble_wait(check: FileCheck, call: ast.Call) -> None:
    if not (check.is_firmware and check.uses_aioble and isinstance(call.func, ast.Attribute)):
        return
    if call.func.attr in BLE_WAITS and not has_keyword(call, "timeout_ms"):
        check.add(call, "PY-30", f"{call.func.attr}() needs timeout_ms=")


def check_call(check: FileCheck, call: ast.Call, scope: Scope) -> None:
    name = qualified(call.func, check.aliases)
    check_suppress(check, call, name)
    check_encoding(check, call, name)
    check_timeout(check, call, name)
    check_patch(check, call, name)
    check_ble_wait(check, call)
    if scope.loops and name in NEW_CONNECTIONS:
        check.add(call, "PY-18", f"{name} inside a loop opens a connection per pass; create one client before the loop")


def reads_wall_clock(check: FileCheck, node: ast.expr, scope: Scope) -> bool:
    if isinstance(node, ast.Call):
        return qualified(node.func, check.aliases) == "time.time"
    return isinstance(node, ast.Name) and node.id in scope.wall_clock_names


def check_statement(check: FileCheck, node: ast.AST, scope: Scope) -> None:
    if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
        if reads_wall_clock(check, node.value, Scope()):
            scope.wall_clock_names.add(node.targets[0].id)
    elif isinstance(node, ast.BinOp) and isinstance(node.op, ast.Sub):
        if reads_wall_clock(check, node.left, scope) and reads_wall_clock(check, node.right, scope):
            check.add(node, "PY-5", "time.time() follows the wall clock; measure durations with time.monotonic()")
    elif isinstance(node, ast.AugAssign) and isinstance(node.op, ast.Add) and scope.loops:
        value = node.value
        is_text = isinstance(value, ast.JoinedStr) or (
            isinstance(value, ast.Constant) and isinstance(value.value, str | bytes)
        )
        rebuilt = isinstance(node.target, ast.Name) and node.target.id in scope.loops[-1]
        if is_text and not rebuilt:
            check.add(node, "PY-16", "build strings with ''.join(parts) or io.StringIO, not += in a loop")
    elif isinstance(node, ast.Call):
        check_call(check, node, scope)
    elif isinstance(node, ast.ExceptHandler):
        check_handler(check, node)


def assigned_names(nodes: list[ast.AST]) -> set[str]:
    return {
        target.id
        for node in nodes
        for inner in ast.walk(node)
        if isinstance(inner, ast.Assign | ast.AnnAssign)
        for target in (inner.targets if isinstance(inner, ast.Assign) else [inner.target])
        if isinstance(target, ast.Name)
    }


def visit_children(check: FileCheck, nodes: list[ast.AST], scope: Scope, loop: set[str] | None) -> None:
    if loop is not None:
        scope.loops.append(loop)
    for child in nodes:
        visit(check, child, scope)
    if loop is not None:
        scope.loops.pop()


def is_fixed_tuple(annotation: ast.expr) -> bool:
    if not isinstance(annotation, ast.Subscript) or not isinstance(annotation.slice, ast.Tuple):
        return False
    items = annotation.slice.elts
    open_ended = any(isinstance(item, ast.Constant) and item.value is Ellipsis for item in items)
    return (
        isinstance(annotation.value, ast.Name) and annotation.value.id == "tuple" and len(items) > 1 and not open_ended
    )


def returns_record_dict(function: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
    annotation = function.returns
    if not (isinstance(annotation, ast.Subscript) and isinstance(annotation.value, ast.Name)):
        return False
    if annotation.value.id != "dict":
        return False
    return any(
        isinstance(node, ast.Return)
        and isinstance(node.value, ast.Dict)
        and len(node.value.keys) > 1
        and all(isinstance(key, ast.Constant) and isinstance(key.value, str) for key in node.value.keys)
        for node in ast.walk(function)
    )


def check_record_returns(check: FileCheck, function: ast.FunctionDef | ast.AsyncFunctionDef) -> None:
    if function.returns is not None and is_fixed_tuple(function.returns):
        check.add(function, "PY-3", "return a dataclass, not a fixed-size tuple")
    elif returns_record_dict(function):
        check.add(function, "PY-3", "return a dataclass or TypedDict, not a dict with fixed keys")


def visit(check: FileCheck, node: ast.AST, scope: Scope) -> None:
    check_statement(check, node, scope)
    children = list(ast.iter_child_nodes(node))
    if isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef):
        check_record_returns(check, node)
    if isinstance(node, FUNCTIONS):
        body: list[ast.AST] = list(node.body) if isinstance(node.body, list) else [node.body]
        visit_children(check, [child for child in children if child not in body], scope, None)
        visit_children(check, body, Scope(), None)
    elif isinstance(node, ast.For | ast.AsyncFor):
        visit_children(check, [node.target, node.iter], scope, None)
        loop_body: list[ast.AST] = [*node.body, *node.orelse]
        visit_children(check, loop_body, scope, assigned_names(loop_body))
    elif isinstance(node, LOOPS):
        visit_children(check, children, scope, assigned_names(children))
    else:
        visit_children(check, children, scope, None)


def is_main_guard(statement: ast.stmt) -> bool:
    if not isinstance(statement, ast.If) or not isinstance(statement.test, ast.Compare):
        return False
    test = statement.test
    left_is_name = isinstance(test.left, ast.Name) and test.left.id == "__name__"
    right = test.comparators[0] if len(test.comparators) == 1 else None
    return left_is_name and isinstance(right, ast.Constant) and right.value == "__main__"


def is_main_exit(statement: ast.stmt) -> bool:
    if not isinstance(statement, ast.Raise) or not isinstance(statement.exc, ast.Call) or statement.cause:
        return False
    outer = statement.exc
    inner = outer.args[0] if len(outer.args) == 1 and not outer.keywords else None
    calls_main = isinstance(inner, ast.Call) and isinstance(inner.func, ast.Name) and inner.func.id == "main"
    plain = calls_main and isinstance(inner, ast.Call) and not inner.args and not inner.keywords
    return isinstance(outer.func, ast.Name) and outer.func.id == "SystemExit" and plain


def runs_unittest(check: FileCheck, statement: ast.stmt) -> bool:
    if not isinstance(statement, ast.Expr) or not isinstance(statement.value, ast.Call):
        return False
    call = statement.value
    return qualified(call.func, check.aliases) == "unittest.main" and not call.args and not call.keywords


def nested_statements(statement: ast.stmt) -> list[ast.stmt] | None:
    if isinstance(statement, ast.If):
        return [*statement.body, *statement.orelse]
    if isinstance(statement, ast.Try):
        handlers = [inner for handler in statement.handlers for inner in handler.body]
        return [*statement.body, *handlers, *statement.orelse, *statement.finalbody]
    return None


def check_top_level(check: FileCheck, statement: ast.stmt) -> None:
    if is_main_guard(statement) and isinstance(statement, ast.If):
        body = statement.body[0] if len(statement.body) == 1 and not statement.orelse else None
        if check.path.name.startswith("test_"):
            if body is None or not runs_unittest(check, body):
                check.add(statement, "PY-8", 'test guard is exactly: if __name__ == "__main__": unittest.main()')
        elif body is None or not is_main_exit(body):
            check.add(statement, "PY-8", 'the guard is exactly: if __name__ == "__main__": raise SystemExit(main())')
        return
    nested = nested_statements(statement)
    if nested is not None:
        for inner in nested:
            check_top_level(check, inner)
        return
    is_literal = isinstance(statement, ast.Expr) and isinstance(statement.value, ast.Constant)
    if not (is_literal or isinstance(statement, TOP_LEVEL_DEFINITIONS) or type(statement).__name__ == "TypeAlias"):
        check.add(statement, "PY-8", "this runs at import time; move it into main()")


def defines_test_case(check: FileCheck, tree: ast.Module) -> bool:
    return any(
        isinstance(node, ast.ClassDef)
        and any(qualified(base, check.aliases).endswith("TestCase") for base in node.bases)
        for node in ast.walk(tree)
    )


def base_errors(check: FileCheck, tree: ast.Module) -> list[ast.ClassDef]:
    return [
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.ClassDef)
        and any(qualified(base, check.aliases) in BROAD_EXCEPTIONS for base in node.bases)
    ]


def parse(path: Path) -> ast.Module | Finding:
    try:
        return ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except (UnicodeDecodeError, SyntaxError) as error:
        return Finding(path, getattr(error, "lineno", None) or 1, 1, "PY-0", f"can't parse: {error}")


def check_file(path: Path, root: Path) -> FileCheck:
    tree = parse(path)
    if isinstance(tree, Finding):
        return FileCheck(path=path, aliases={}, is_firmware=False, uses_aioble=False, findings=[tree])
    aliases = import_aliases(tree)
    parts = path.relative_to(root).parts
    uses_aioble = any(name.startswith("aioble") for name in aliases.values())
    check = FileCheck(path=path, aliases=aliases, is_firmware="firmware" in parts, uses_aioble=uses_aioble)
    visit(check, tree, Scope())
    if not check.is_firmware and path.name != "__main__.py":
        for statement in tree.body:
            check_top_level(check, statement)
    in_tests = "tests" in parts[:-1]
    if in_tests and not path.name.startswith("test_") and defines_test_case(check, tree):
        check.add_at(1, 0, "PY-25", "a file with TestCase classes is named tests/test_<module>.py")
    if not in_tests:
        check.base_errors.extend(base_errors(check, tree))
    return check


def vendored_list() -> Path:
    here = Path(__file__).resolve().parent
    for place in VENDORED_PLACES:
        listed = (here / place / VENDORED_FILE).resolve()
        if listed.is_file():
            return listed
    message = f"{VENDORED_FILE} is missing next to {Path(__file__).name}. Run Install-Lint again."
    raise FileNotFoundError(message)


def vendored_entries() -> list[str]:
    lines = vendored_list().read_text(encoding="utf-8").splitlines()
    return [line.strip() for line in lines if line.strip()]


def is_vendored(relative: str, entries: list[str]) -> bool:
    parts = relative.split("/")
    for entry in entries:
        name = entry.strip("/")
        if entry.startswith("/"):
            if relative.startswith(name + "/"):
                return True
        elif name in parts[:-1]:
            return True
    return False


def tracked_files(root: Path) -> list[str]:
    git = shutil.which("git")
    if git is None:
        message = "git isn't on PATH"
        raise OSError(message)
    result = subprocess.run(
        [git, "-C", str(root), "ls-files", "-z", "--", "*.py", "*pyproject.toml"],
        capture_output=True,
        check=True,
        timeout=120,
    )
    return [entry for entry in result.stdout.decode("utf-8", errors="replace").split("\0") if entry]


def walk(root: Path) -> Listing:
    if root.is_file():
        return Listing([root], [])
    entries = vendored_entries()
    kept = [root / name for name in tracked_files(root) if not is_vendored(name, entries)]
    sources = sorted(path for path in kept if path.suffix == ".py" and path.is_file())
    projects = sorted(path for path in kept if path.name == "pyproject.toml" and path.is_file())
    return Listing(sources, projects)


def is_package(directory: Path) -> bool:
    return (directory / "__init__.py").is_file()


def top_package(path: Path, root: Path) -> Path | None:
    found: Path | None = None
    directory = path.parent
    while directory not in {root, directory.parent} and is_package(directory):
        found = directory
        directory = directory.parent
    return found


def check_names(sources: list[Path], root: Path) -> list[Finding]:
    findings: list[Finding] = []
    packages: set[Path] = set()
    for path in sources:
        if not MODULE_NAME.fullmatch(path.stem):
            findings.append(Finding(path, 1, 1, "PY-27", f"module name {path.stem!r} is lowercase with underscores"))
        directory = path.parent
        while directory not in {root, directory.parent} and is_package(directory):
            packages.add(directory)
            directory = directory.parent
    findings.extend(
        Finding(package / "__init__.py", 1, 1, "PY-27", f"package name {package.name!r} is lowercase with underscores")
        for package in sorted(packages)
        if not MODULE_NAME.fullmatch(package.name)
    )
    return findings


def check_layout(projects: list[Path]) -> list[Finding]:
    findings: list[Finding] = []
    for project in projects:
        if not BUILD_SYSTEM.search(project.read_text(encoding="utf-8")):
            continue
        findings.extend(
            Finding(child / "__init__.py", 1, 1, "PY-24", f"installable package {child.name!r} goes under src/")
            for child in sorted(project.parent.iterdir())
            if child.is_dir() and child.name not in {"src", "tests"} and is_package(child)
        )
    return findings


def check_src_packages(sources: list[Path], root: Path) -> list[Finding]:
    missing: set[Path] = set()
    for path in sources:
        if "src" not in path.relative_to(root).parts[:-1]:
            continue
        directory = path.parent
        while directory.name != "src" and directory != root:
            if not is_package(directory):
                missing.add(directory)
            directory = directory.parent
    return [
        Finding(directory, 1, 1, "PY-24", "a folder of modules under src/ is a package; add __init__.py")
        for directory in sorted(missing)
    ]


def check_base_errors(packages: dict[Path, list[FileCheck]]) -> list[Finding]:
    return [
        Finding(
            check.path, node.lineno, node.col_offset + 1, "PY-10", f"{package.name} has one base error; subclass it"
        )
        for package, checks in packages.items()
        if sum(len(check.base_errors) for check in checks) > 1
        for check in checks
        for node in check.base_errors
    ]


def check_tree(root: Path) -> list[Finding]:
    listing = walk(root)
    return check_paths(listing.sources, listing.projects, root if root.is_dir() else root.parent)


def check_paths(sources: list[Path], projects: list[Path], base: Path) -> list[Finding]:
    findings = check_names(sources, base) + check_layout(projects) + check_src_packages(sources, base)
    packages: dict[Path, list[FileCheck]] = {}
    for path in sources:
        check = check_file(path, base)
        findings.extend(check.findings)
        package = top_package(path, base)
        if package is not None and check.base_errors:
            packages.setdefault(package, []).append(check)
    return findings + check_base_errors(packages)


def main() -> int:
    roots = [Path(argument).resolve() for argument in sys.argv[1:]] or [Path.cwd()]
    try:
        findings = [finding for root in roots for finding in check_tree(root)]
    except (subprocess.CalledProcessError, OSError) as error:
        sys.stderr.write(f"can't list the files to check: {error}\n")
        return 2
    for finding in findings:
        sys.stdout.write(finding.render() + "\n")
    if findings:
        sys.stdout.write(f"{len(findings)} findings\n")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
