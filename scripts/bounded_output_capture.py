#!/usr/bin/env python3
"""Keep the tail of stdin in one private file."""

import os
import sys
import tempfile
from pathlib import Path


MAX_BYTES = 64 * 1024


def write_snapshot(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary_path: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=path.parent,
            prefix=f".{path.name}.",
            delete=False,
        ) as temporary:
            temporary_path = temporary.name
            os.chmod(temporary.fileno(), 0o600)
            temporary.write(data)
            temporary.flush()
            os.fsync(temporary.fileno())
        os.replace(temporary_path, path)
        temporary_path = None
    finally:
        if temporary_path is not None:
            Path(temporary_path).unlink(missing_ok=True)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: bounded_output_capture.py OUTPUT", file=sys.stderr)
        return 2
    output = Path(sys.argv[1])
    tail = bytearray()
    write_snapshot(output, tail)
    while chunk := sys.stdin.buffer.read1(8192):
        tail.extend(chunk)
        del tail[:-MAX_BYTES]
        write_snapshot(output, tail)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
