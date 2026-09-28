#!/usr/bin/env python3
"""Parse Phoenix release versions and derive release-platform versions."""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass

_NUMBER = r"(?:0|[1-9][0-9]*)"
_VERSION_RE = re.compile(rf"({_NUMBER})\.({_NUMBER})\.({_NUMBER})(?:-rc\.({_NUMBER}))?")
_RC_APPLE_CUTOFF = (0, 13, 0)


@dataclass(frozen=True)
class ReleaseVersion:
    major: int
    minor: int
    patch: int
    rc: int | None = None

    def __post_init__(self) -> None:
        values = (self.major, self.minor, self.patch)
        if any(isinstance(value, bool) or not isinstance(value, int) for value in values):
            raise ValueError("version components must be integers")
        if self.rc is not None and (isinstance(self.rc, bool) or not isinstance(self.rc, int)):
            raise ValueError("release candidate number must be an integer")
        if any(value < 0 for value in values):
            raise ValueError("version components cannot be negative")
        if self.major >= 9_999 or self.minor >= 100 or self.patch >= 99:
            raise ValueError("version components exceed release bounds")
        if self.rc is not None and not 1 <= self.rc <= 98:
            raise ValueError("release candidate number must be between 1 and 98")
        if self.rc is not None and values < _RC_APPLE_CUTOFF:
            raise ValueError("release candidates require version 0.13.0 or newer")

    @classmethod
    def parse(cls, value: str) -> "ReleaseVersion":
        match = _VERSION_RE.fullmatch(value)
        if match is None:
            raise ValueError("version must be X.Y.Z or X.Y.Z-rc.N")
        major, minor, patch = (int(part) for part in match.group(1, 2, 3))
        rc = int(match.group(4)) if match.group(4) is not None else None
        return cls(major, minor, patch, rc)

    def __str__(self) -> str:
        base = f"{self.major}.{self.minor}.{self.patch}"
        return f"{base}-rc.{self.rc}" if self.rc is not None else base

    def next_default(self) -> "ReleaseVersion":
        if self.rc is not None:
            if self.rc == 98:
                raise ValueError("release candidate number cannot exceed 98")
            return ReleaseVersion(self.major, self.minor, self.patch, self.rc + 1)
        if self.patch == 98:
            raise ValueError("patch number cannot exceed 98")
        return ReleaseVersion(self.major, self.minor, self.patch + 1)

    def apple_marketing_version(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"

    def apple_build_version(self) -> str:
        if (self.major, self.minor, self.patch) < _RC_APPLE_CUTOFF:
            return f"{self.major + 1}.{self.minor}.{self.patch}"
        suffix = self.rc if self.rc is not None else 99
        return f"{self.major + 1}.{self.minor}.{self.patch * 100 + suffix}"


def parse_tag(value: str) -> ReleaseVersion:
    if not value.startswith("v"):
        raise ValueError("release tag must start with v")
    return ReleaseVersion.parse(value[1:])


def version_key(version: ReleaseVersion) -> tuple[int, int, int, bool, int]:
    return (
        version.major,
        version.minor,
        version.patch,
        version.rc is None,
        version.rc or 0,
    )


def latest_supported(tags: list[str]) -> ReleaseVersion | None:
    versions = []
    for tag in tags:
        try:
            versions.append(parse_tag(tag))
        except ValueError:
            continue
    if not versions:
        return None
    return max(versions, key=version_key)


def _main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "command",
        choices=(
            "validate",
            "validate-tag",
            "validate-new-from-tags",
            "next",
            "next-from-tags",
            "channel",
            "apple-marketing",
            "apple-build",
        ),
    )
    parser.add_argument("version", nargs="?")
    args = parser.parse_args()
    try:
        if args.command in ("next-from-tags", "validate-new-from-tags"):
            latest = latest_supported([line.strip() for line in sys.stdin if line.strip()])
            if args.command == "next-from-tags":
                print(str(latest.next_default()) if latest is not None else "0.1.0")
                return 0
            if args.version is None:
                parser.error("version is required")
            version = ReleaseVersion.parse(args.version)
            if latest is not None and version_key(version) <= version_key(latest):
                raise ValueError(f"new version must be newer than released version {latest}")
            print(str(version))
            return 0
        if args.version is None:
            parser.error("version is required")
        version = (
            parse_tag(args.version)
            if args.command == "validate-tag"
            else ReleaseVersion.parse(args.version)
        )
        if args.command in ("validate", "validate-tag"):
            result = str(version)
        elif args.command == "next":
            result = str(version.next_default())
        elif args.command == "channel":
            result = "rc" if version.rc is not None else "stable"
        elif args.command == "apple-marketing":
            result = version.apple_marketing_version()
        else:
            result = version.apple_build_version()
    except ValueError as error:
        parser.error(str(error))
    print(result)
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
