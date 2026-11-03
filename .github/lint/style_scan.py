from __future__ import annotations

import bisect
import re
from dataclasses import dataclass, field

RAW_QUOTE_RUN = 3


@dataclass(frozen=True, slots=True, order=True)
class Finding:
    path: str
    line: int
    message: str

    def render(self) -> str:
        if self.line:
            return f"{self.path}:{self.line}: {self.message}"
        return f"{self.path}: {self.message}"


@dataclass(frozen=True, slots=True)
class Comment:
    line: int
    text: str


@dataclass(slots=True)
class Scan:
    text: str
    code: list[str] = field(default_factory=list[str])
    comments: list[Comment] = field(default_factory=list[Comment])
    directives: list[Comment] = field(default_factory=list[Comment])
    starts: list[int] = field(default_factory=list[int])

    def line_of(self, index: int) -> int:
        return bisect.bisect_right(self.starts, index)


class Lexer:
    def __init__(self, text: str, *, rust: bool) -> None:
        self.text = text
        self.rust = rust
        self.scan = Scan(text, list(text), [], [], [0] + [m.end() for m in re.finditer(r"\n", text)])

    def run(self) -> Scan:
        self.code(0, inside_interpolation=False)
        return self.scan

    def blank(self, start: int, end: int) -> None:
        code = self.scan.code
        for index in range(start, min(end, len(code))):
            if code[index] != "\n":
                code[index] = " "

    def comment(self, start: int, end: int) -> None:
        self.scan.comments.append(Comment(self.scan.line_of(start), self.text[start:end]))
        self.blank(start, end)

    def code(self, index: int, *, inside_interpolation: bool) -> int:
        text = self.text
        size = len(text)
        depth = 0
        while index < size:
            char = text[index]
            following = text[index + 1] if index + 1 < size else ""
            if char == "/" and following == "/":
                end = text.find("\n", index)
                end = size if end < 0 else end
                self.comment(index, end)
                index = end
                continue
            if char == "/" and following == "*":
                end = self.block_comment_end(index)
                self.comment(index, end)
                index = end
                continue
            if not self.rust and char == "#" and self.at_line_start(index):
                end = text.find("\n", index)
                end = size if end < 0 else end
                self.scan.directives.append(Comment(self.scan.line_of(index), text[index:end]))
                index = end
                continue
            literal_end = self.rust_literal(index) if self.rust else self.csharp_literal(index)
            if literal_end is not None:
                index = literal_end
                continue
            if char == "{":
                depth += 1
            elif char == "}":
                if depth == 0 and inside_interpolation:
                    return index
                depth -= 1
            index += 1
        return size

    def at_line_start(self, index: int) -> bool:
        start = self.text.rfind("\n", 0, index) + 1
        return not self.text[start:index].strip()

    def block_comment_end(self, index: int) -> int:
        text = self.text
        depth = 0
        while index < len(text):
            pair = text[index : index + 2]
            if pair == "/*":
                depth += 1
                index += 2
                if not self.rust:
                    end = text.find("*/", index)
                    return len(text) if end < 0 else end + 2
                continue
            if pair == "*/":
                depth -= 1
                index += 2
                if depth == 0:
                    return index
                continue
            index += 1
        return len(text)

    def quoted(self, start: int, index: int, *, escapes: bool, verbatim: bool) -> int:
        text = self.text
        while index < len(text):
            char = text[index]
            if escapes and char == "\\":
                index += 2
                continue
            if char == '"':
                if verbatim and text[index + 1 : index + 2] == '"':
                    index += 2
                    continue
                index += 1
                break
            index += 1
        end = min(index, len(text))
        self.blank(start, end)
        return end

    def char_literal(self, index: int) -> int | None:
        text = self.text
        if text[index + 1 : index + 2] == "\\":
            end = text.find("'", index + 3)
            end = len(text) if end < 0 else end + 1
        elif text[index + 2 : index + 3] == "'":
            end = index + 3
        else:
            return None
        self.blank(index, end)
        return end

    def rust_literal(self, index: int) -> int | None:
        text = self.text
        start = index
        previous = text[index - 1] if index else " "
        starts_token = not (previous.isalnum() or previous == "_")
        if starts_token and text[index] in "bc":
            index += 1
        if starts_token and text[index : index + 1] == "r":
            return self.raw_string(start, index + 1)
        if text[index : index + 1] == '"':
            return self.quoted(start, index + 1, escapes=True, verbatim=False)
        if text[index : index + 1] == "'":
            return self.char_literal(index)
        return None

    def raw_string(self, start: int, index: int) -> int | None:
        text = self.text
        opening = index
        while text[index : index + 1] == "#":
            index += 1
        if text[index : index + 1] != '"':
            return None
        close = '"' + "#" * (index - opening)
        end = text.find(close, index + 1)
        end = len(text) if end < 0 else end + len(close)
        self.blank(start, end)
        return end

    def csharp_literal(self, index: int) -> int | None:
        text = self.text
        if text[index] == "'":
            return self.char_literal(index)
        cursor = index
        dollars = 0
        verbatim = False
        while cursor < len(text) and text[cursor] in "$@":
            if text[cursor] == "$":
                dollars += 1
            else:
                verbatim = True
            cursor += 1
        if text[cursor : cursor + 1] != '"':
            return None
        quotes = len(text[cursor:]) - len(text[cursor:].lstrip('"'))
        if quotes >= RAW_QUOTE_RUN:
            close = '"' * quotes
            end = text.find(close, cursor + quotes)
            end = len(text) if end < 0 else end + quotes
            self.blank(index, end)
            return end
        if not dollars:
            return self.quoted(index, cursor + 1, escapes=not verbatim, verbatim=verbatim)
        return self.interpolated(index, cursor + 1, verbatim=verbatim)

    def interpolated(self, start: int, index: int, *, verbatim: bool) -> int:
        text = self.text
        while index < len(text):
            char = text[index]
            pair = text[index : index + 2]
            if not verbatim and char == "\\":
                index += 2
                continue
            if char == '"':
                if verbatim and pair == '""':
                    index += 2
                    continue
                index += 1
                break
            if pair in {"{{", "}}"}:
                index += 2
                continue
            if char == "{":
                self.blank(start, index + 1)
                index = self.code(index + 1, inside_interpolation=True)
                start = index
                index += 1
                continue
            index += 1
        end = min(index, len(text))
        self.blank(start, end)
        return end


def find_closing_brace(code: str, index: int) -> int:
    depth = 0
    for position in range(index, len(code)):
        if code[position] == "{":
            depth += 1
        elif code[position] == "}":
            depth -= 1
            if depth == 0:
                return position
    return len(code) - 1


def first_code_line(scan: Scan) -> int:
    code = "".join(scan.code)
    match = re.search(r"\S", code)
    return scan.line_of(match.start()) if match else len(scan.starts) + 1


def count_lines(text: str) -> int:
    if not text:
        return 0
    return text.count("\n") + (0 if text.endswith("\n") else 1)
