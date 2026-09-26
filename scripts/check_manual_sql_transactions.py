#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Reject unowned SQL transactions in production Rust SQL literals."""

from __future__ import annotations

import argparse
import json
import re
import sqlite3
import subprocess
import sys

RULE = r'''
id: manual-sql-transaction-candidate
language: Rust
severity: warning
rule:
  all:
    - any:
        - kind: string_literal
        - kind: raw_string_literal
    - pattern: $SQL
    - inside:
        kind: arguments
        stopBy:
          not:
            kind: parenthesized_expression
        inside:
          kind: call_expression
          has:
            field: function
            regex: '(?s)(^|::|\.)\s*(?:r#)?(query|query_scalar|query_as|query_with|query_scalar_with|query_as_with|raw_sql|execute|execute_batch|execute_many|exec|fetch|fetch_all|fetch_one|fetch_optional|fetch_many)\s*(::\s*<.*>)?$'
    - not:
        inside:
          all:
            - any:
                - kind: mod_item
                - kind: function_item
                - kind: impl_item
            - follows:
                kind: attribute_item
                regex: '^#\[\s*(cfg\(\s*test\s*\)|(?:tokio::)?test)\s*\]$'
                stopBy:
                  not:
                    kind: attribute_item
          stopBy: end
'''

ESCAPE = re.compile(r'''\\(?:[nrt0\\'"]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|\n[ \t\n\r]*)''')
SIMPLE_ESCAPES = {"n": "\n", "r": "\r", "t": "\t", "0": "\0",
                  "\\": "\\", "'": "'", '"': '"'}
LEADING_COMMENTS = re.compile(r"(?:\s|\ufeff|--[^\n]*(?:\n|$)|/\*.*?(?:\*/|$))*", re.DOTALL)
CONTROL = re.compile(r"(BEGIN|COMMIT|END|ROLLBACK|SAVEPOINT|RELEASE)\b", re.IGNORECASE)


def decode_rust_string(literal: str) -> str:
    opening = literal.index('"')
    body = literal[opening + 1:literal.rindex('"')].replace("\r\n", "\n")
    if literal.startswith("r"):
        return body
    result = []
    offset = 0
    while offset < len(body):
        if body[offset] != "\\":
            result.append(body[offset])
            offset += 1
            continue
        escape = ESCAPE.match(body, offset)
        if escape is None:
            raise ValueError("unsupported Rust string escape")
        value = escape.group()[1:]
        if value in SIMPLE_ESCAPES:
            result.append(SIMPLE_ESCAPES[value])
        elif value.startswith("x"):
            result.append(chr(int(value[1:], 16)))
        elif value.startswith("u"):
            result.append(chr(int(value[2:-1].replace("_", ""), 16)))
        # A backslash followed by a physical newline is a Rust continuation.
        offset = escape.end()
    return "".join(result)


def transaction_controls(sql: str) -> list[str]:
    statements = []
    start = 0
    for offset, char in enumerate(sql):
        if char == ";" and sqlite3.complete_statement(sql[start:offset + 1]):
            statements.append(sql[start:offset + 1])
            start = offset + 1
    statements.append(sql[start:])
    controls = []
    for statement in statements:
        prefix = LEADING_COMMENTS.match(statement).end()
        control = CONTROL.match(statement, prefix)
        if control:
            controls.append(control.group().upper())
    return controls


def findings(paths: list[str], source: str | None = None) -> list[dict]:
    command = ["ast-grep", "scan", "--inline-rules", RULE, "--json=compact"]
    command.extend(["--stdin"] if source is not None else paths)
    result = subprocess.run(command, input=source, text=True, capture_output=True, check=False)
    if result.returncode not in (0, 1):
        raise RuntimeError(result.stderr.strip() or "ast-grep failed")
    found = []
    for match in json.loads(result.stdout):
        literal = match["metaVariables"]["single"]["SQL"]["text"]
        controls = transaction_controls(decode_rust_string(literal))
        if controls:
            location = match["range"]["start"]
            found.append({"file": match["file"], "line": location["line"] + 1,
                          "column": location["column"] + 1, "controls": controls})
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", default=["crates/"])
    parser.add_argument("--stdin", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    try:
        found = findings(args.paths, sys.stdin.read() if args.stdin else None)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"manual-sql-transactions: {error}", file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(found))
    else:
        for item in found:
            print(f"{item['file']}:{item['line']}:{item['column']}: "
                  f"manual SQL transaction control ({', '.join(item['controls'])}); "
                  "use SQLx begin()/begin_with() and the owned transaction's commit()/rollback()")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
