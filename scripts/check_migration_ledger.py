#!/usr/bin/env python3
"""Validate migration-ledger structure without compiling Rust."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_LEDGER = ROOT / "crates" / "phoenix-db" / "src" / "migrations.rs"
_NAME = re.compile(r"^[a-z0-9_]+$")
_NUMBERED_SQL = re.compile(r"^MIGRATION_(\d{3})$")
Token = tuple[str, str]

_CHAR = re.compile(
    r"(?:b)?'(?:[^'\\\n]|\\(?:[nrt0\\'\"]|x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f_]{1,6}\}))'"
)
_BRACKETS = {
    ("{", "{"): ("}", "}"),
    ("[", "["): ("]", "]"),
    ("(", "("): (")", ")"),
}
_CLOSING_BRACKETS = set(_BRACKETS.values())


def _identifier_end(source: str, index: int) -> int:
    while index < len(source) and ("a" + source[index]).isidentifier():
        index += 1
    return index


def _split_top_level(tokens: list[Token]) -> tuple[list[list[Token]], list[str]]:
    parts: list[list[Token]] = []
    current: list[Token] = []
    stack: list[Token] = []
    errors: list[str] = []
    for token in tokens:
        if token in _BRACKETS:
            stack.append(_BRACKETS[token])
        elif token in _CLOSING_BRACKETS:
            if not stack or stack.pop() != token:
                errors.append("unbalanced delimiter")
                return [], errors
        if token == (",", ",") and not stack:
            if not current:
                errors.append("empty comma-delimited item")
                return [], errors
            parts.append(current)
            current = []
        else:
            current.append(token)
    if stack:
        errors.append("unbalanced delimiter")
    if current:
        parts.append(current)
    return parts, errors


def _entry_fields(entry: list[Token]) -> tuple[dict[str, list[Token]], list[str]]:
    parts, errors = _split_top_level(entry)
    fields: dict[str, list[Token]] = {}
    for part in parts:
        if len(part) < 3 or part[0][0] != "IDENT" or part[1] != (":", ":"):
            errors.append("migration entry has an unsupported field shape")
            continue
        name = part[0][1]
        if name in fields:
            errors.append(f"migration entry repeats field {name}")
            continue
        fields[name] = part[2:]
    return fields, errors


def _single_token(fields: dict[str, list[Token]], name: str) -> Token | None:
    value = fields.get(name)
    return value[0] if value is not None and len(value) == 1 else None


def _decimal_literal(token: Token | None) -> int | None:
    if token is None or token[0] != "NUMBER" or not re.fullmatch(r"[0-9][0-9_]*", token[1]):
        return None
    return int(token[1].replace("_", ""))


def _numbered_sql_references(tokens: list[Token]) -> list[tuple[str, int]]:
    references: list[tuple[str, int]] = []
    for kind, value in tokens:
        if kind != "IDENT":
            continue
        match = _NUMBERED_SQL.fullmatch(value)
        if match:
            references.append((value, int(match.group(1))))
    return references


def _rust_tokens(source: str) -> list[Token]:
    tokens: list[Token] = []
    index = 0
    length = len(source)
    while index < length:
        char = source[index]
        if char.isspace():
            index += 1
            continue
        if source.startswith("//", index):
            newline = source.find("\n", index + 2)
            index = length if newline == -1 else newline + 1
            continue
        if source.startswith("/*", index):
            depth = 1
            index += 2
            while index < length and depth:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            if depth:
                raise ValueError("unterminated block comment")
            continue

        char_match = _CHAR.match(source, index)
        if char_match:
            tokens.append(("CHAR", char_match.group(0)))
            index = char_match.end()
            continue

        raw_match = re.match(r"(?:b|c)?r(#+)?\"", source[index:])
        if raw_match:
            hashes = raw_match.group(1) or ""
            content_start = index + raw_match.end()
            terminator = '"' + hashes
            content_end = source.find(terminator, content_start)
            if content_end == -1:
                raise ValueError("unterminated raw string")
            tokens.append(("STRING", source[content_start:content_end]))
            index = content_end + len(terminator)
            continue

        string_prefix = 1 if source.startswith(('b"', 'c"'), index) else 0
        if source[index + string_prefix : index + string_prefix + 1] == '"':
            content_start = index + string_prefix + 1
            cursor = content_start
            while cursor < length:
                if source[cursor] == "\\":
                    cursor += 2
                    continue
                if source[cursor] == '"':
                    break
                cursor += 1
            if cursor >= length:
                raise ValueError("unterminated string")
            tokens.append(("STRING", source[content_start:cursor]))
            index = cursor + 1
            continue

        if char.isalpha() or char == "_":
            end = _identifier_end(source, index + 1)
            tokens.append(("IDENT", source[index:end]))
            index = end
            continue
        if char.isdigit():
            end = _identifier_end(source, index + 1)
            tokens.append(("NUMBER", source[index:end]))
            index = end
            continue

        tokens.append((char, char))
        index += 1
    return tokens


def _migration_entries(tokens: list[Token]) -> tuple[list[list[Token]], list[str]]:
    errors: list[str] = []
    starts = [
        index
        for index, token in enumerate(tokens)
        if token == ("IDENT", "MIGRATIONS")
        and index > 0
        and tokens[index - 1] == ("IDENT", "const")
    ]
    if len(starts) != 1:
        return [], ["expected exactly one const MIGRATIONS declaration"]

    cursor = starts[0]
    declaration = [
        (":", ":"),
        ("&", "&"),
        ("[", "["),
        ("IDENT", "Migration"),
        ("]", "]"),
        ("=", "="),
        ("&", "&"),
        ("[", "["),
    ]
    if tokens[cursor + 1 : cursor + 9] != declaration:
        return [], ["MIGRATIONS declaration is not a literal borrowed Migration array"]
    cursor += 8
    array_start = cursor
    stack: list[Token] = [("]", "]")]
    cursor += 1
    while cursor < len(tokens) and stack:
        token = tokens[cursor]
        if token in _BRACKETS:
            stack.append(_BRACKETS[token])
        elif token in _CLOSING_BRACKETS:
            if stack.pop() != token:
                return [], ["unbalanced delimiter in MIGRATIONS array"]
        cursor += 1
    if stack:
        return [], ["unterminated MIGRATIONS array"]

    if cursor >= len(tokens) or tokens[cursor] != (";", ";"):
        return [], ["MIGRATIONS initializer has unsupported trailing tokens"]

    elements, split_errors = _split_top_level(tokens[array_start + 1 : cursor - 1])
    errors.extend(split_errors)
    entries: list[list[Token]] = []
    for ordinal, element in enumerate(elements, start=1):
        if (
            len(element) < 3
            or element[:2] != [("IDENT", "Migration"), ("{", "{")]
            or element[-1] != ("}", "}")
        ):
            errors.append(f"ledger element {ordinal} is not a literal Migration entry")
            continue
        entries.append(element[2:-1])

    if not elements:
        errors.append("migration ledger is empty")
    return entries, errors


def validate_migration_ledger(source: str) -> list[str]:
    try:
        entries, errors = _migration_entries(_rust_tokens(source))
    except ValueError as error:
        return [str(error)]

    parsed: list[tuple[int, str, list[Token]]] = []
    for ordinal, entry in enumerate(entries, start=1):
        fields, field_errors = _entry_fields(entry)
        errors.extend(f"entry {ordinal}: {error}" for error in field_errors)
        version = _decimal_literal(_single_token(fields, "version"))
        name = _single_token(fields, "name")
        sql = fields.get("sql")
        if version is None:
            errors.append(f"entry {ordinal} has no literal decimal version")
            continue
        if name is None or name[0] != "STRING" or not _NAME.fullmatch(name[1]):
            errors.append(f"migration {version} has no snake-case literal name")
            continue
        if not sql:
            errors.append(f"migration {version} has no sql value")
            continue
        parsed.append((version, name[1], sql))

    versions = [version for version, _, _ in parsed]
    names = [name for _, name, _ in parsed]
    for previous, current in zip(versions, versions[1:]):
        if current <= previous:
            errors.append(
                "migration versions must be strictly increasing: "
                f"{previous} is followed by {current}"
            )

    duplicate_versions = sorted(
        version for version in set(versions) if versions.count(version) > 1
    )
    if duplicate_versions:
        errors.append(f"duplicate migration versions: {duplicate_versions}")

    duplicate_names = sorted(name for name in set(names) if names.count(name) > 1)
    if duplicate_names:
        errors.append(f"duplicate migration names: {duplicate_names}")

    for version, _, sql in parsed:
        for reference, referenced_version in _numbered_sql_references(sql):
            if referenced_version != version:
                errors.append(f"migration {version} references {reference}")

    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", nargs="?", type=Path, default=DEFAULT_LEDGER)
    args = parser.parse_args(argv)

    errors = validate_migration_ledger(args.path.read_text())
    if errors:
        for error in errors:
            print(f"migration ledger: {error}", file=sys.stderr)
        return 1
    print(f"migration ledger: OK ({args.path})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
