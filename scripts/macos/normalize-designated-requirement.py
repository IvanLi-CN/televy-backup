#!/usr/bin/env python3
"""Reconstruct and normalize the designated requirement emitted by codesign."""

import re
import sys


def normalize_space(value: str) -> str:
    result: list[str] = []
    pending_space = False
    in_quote = False
    escaped = False
    for character in value:
        if in_quote:
            result.append(character)
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                in_quote = False
        elif character == '"':
            if pending_space and result:
                result.append(" ")
                pending_space = False
            result.append(character)
            in_quote = True
        elif character.isspace():
            pending_space = True
        else:
            if pending_space and result:
                result.append(" ")
            pending_space = False
            result.append(character)
    return "".join(result).strip()


def main() -> int:
    lines = sys.stdin.read().splitlines()
    marker = "designated =>"
    for index, line in enumerate(lines):
        if marker not in line:
            continue
        parts = [line[line.index(marker):]]
        for continuation in lines[index + 1:]:
            if continuation.strip() and not continuation[0].isspace():
                break
            if continuation.strip():
                parts.append(continuation)
        requirement = normalize_space(" ".join(part.strip() for part in parts if part.strip()))
        requirement = re.sub(r"^#\s*", "", requirement)
        cdhash_term = r'cdhash\s+H"([0-9A-Fa-f]+)"'
        cdhash_or = re.compile(rf"{cdhash_term}(?:\s+or\s+{cdhash_term})+")

        def sort_cdhash_alternatives(match: re.Match[str]) -> str:
            values = re.findall(cdhash_term, match.group(0))
            return " or ".join(f'cdhash H"{value.lower()}"' for value in sorted(values, key=str.lower))

        print(cdhash_or.sub(sort_cdhash_alternatives, requirement))
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
