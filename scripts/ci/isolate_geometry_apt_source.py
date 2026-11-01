"""Isolate the unrelated Microsoft Noble feed before CI's Playwright apt install."""

import contextlib
import os
from pathlib import Path
import re
import stat
import sys


TARGET = "https://packages.microsoft.com/ubuntu/24.04/prod"
TARGETS = {TARGET, TARGET + "/"}
URL = re.compile(r"https://[^\s\[\]<>\"']+")
FIELD = re.compile(r"([A-Za-z0-9][A-Za-z0-9-]*):[ \t]*")
WORD = re.compile(r"[A-Za-z0-9_./+()-]+")


def contains_target(text):
    return any(match.group() in TARGETS for match in URL.finditer(text))


def invalid():
    raise ValueError("unexpected target source shape")


def isolate_list(text):
    result = []
    isolated = 0
    for line in text.splitlines(keepends=True):
        active = re.split(r"(?<!\S)#", line, maxsplit=1)[0].strip()
        if not contains_target(active):
            result.append(line)
            continue
        match = re.fullmatch(r"(deb|deb-src)\s+(?:\[([^\[\]]*)\]\s+)?(\S+)\s+(\S+)(?:\s+(.*))?", active)
        if not match:
            invalid()
        _, options, uri, suite, components = match.groups()
        if uri not in TARGETS or not WORD.fullmatch(suite):
            invalid()
        if options is not None and (not options.split() or any(
            not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9-]*[+-]?=[^\s\[\]\"']+", option)
            for option in options.split()
        )):
            invalid()
        if ((suite.endswith("/") and components) or (not suite.endswith("/") and not components)
                or any(not WORD.fullmatch(component) for component in (components or "").split())):
            invalid()
        isolated += 1
    return "".join(result), isolated


def isolate_stanza(lines):
    active = [line for line in lines if not line.lstrip().startswith("#")]
    if not contains_target("".join(active)):
        return "".join(lines), 0
    fields = {}
    current = None
    for index, line in enumerate(lines):
        if line.lstrip().startswith("#"):
            continue
        if line.startswith((" ", "\t")):
            if current is None:
                invalid()
            fields[current].append((index, 0))
        else:
            match = FIELD.match(line)
            if not match:
                invalid()
            current = match[1].lower()
            if current in fields:
                invalid()
            fields[current] = [(index, match.end())]

    def tokens(name):
        return [token for index, start in fields.get(name, [])
                for token in lines[index][start:].split()]

    enabled = tokens("enabled")
    if "enabled" in fields and enabled not in (["yes"], ["no"]):
        invalid()
    if enabled == ["no"]:
        return "".join(lines), 0
    uris = tokens("uris")
    if (not any(uri in TARGETS for uri in uris)
            or any(not re.fullmatch(r"[A-Za-z][A-Za-z0-9+.-]*:\S+", uri) for uri in uris)):
        invalid()
    if any(contains_target(lines[index][start:]) for name, parts in fields.items()
           if name != "uris" for index, start in parts):
        invalid()
    if not tokens("types") or any(token not in ("deb", "deb-src") for token in tokens("types")):
        invalid()
    suites = tokens("suites")
    components = tokens("components")
    if not suites or any(not WORD.fullmatch(token) for token in suites + components):
        invalid()
    if ((any(suite.endswith("/") for suite in suites) and components)
            or (any(not suite.endswith("/") for suite in suites) and not components)):
        invalid()
    # A bracketed/quoted exact target is not a URI token; do not guess its meaning.
    if any(contains_target(uri) and uri not in TARGETS for uri in uris):
        invalid()
    if all(uri in TARGETS for uri in uris):
        if "enabled" in fields:
            # Enabled may be folded onto a continuation line.
            for index, start in fields["enabled"]:
                if "yes" in lines[index][start:].split():
                    lines[index] = lines[index][:start] + re.sub(r"\byes\b", "no", lines[index][start:], count=1)
                    break
        else:
            newline = "\r\n" if lines[0].endswith("\r\n") else "\n"
            lines.insert(0, "Enabled: no" + newline)
    else:
        for index, start in fields["uris"]:
            value = re.sub(r"\S+", lambda m: "" if m[0] in TARGETS else m[0], lines[index][start:])
            # Empty continuation lines could split the stanza in apt's parser.
            lines[index] = "" if start == 0 and not value.strip() else lines[index][:start] + value
    return "".join(lines), 1


def isolate_sources(text):
    result = []
    stanza = []
    isolated = 0
    for line in text.splitlines(keepends=True):
        if line.strip():
            stanza.append(line)
        else:
            updated, count = isolate_stanza(stanza)
            result.extend((updated, line))
            isolated += count
            stanza = []
    updated, count = isolate_stanza(stanza)
    result.append(updated)
    return "".join(result), isolated + count


def fingerprint(info):
    # Reading may update atime, which is not a source change.
    return (info.st_dev, info.st_ino, info.st_mode, info.st_nlink,
            info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def regular_file(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise ValueError("unexpected source file type")
    return info


def isolate(apt_dir):
    """Tests pass a temporary apt directory; the CLI always uses /etc/apt."""
    if (not apt_dir.is_absolute() or apt_dir.resolve() != apt_dir
            or not stat.S_ISDIR(apt_dir.lstat().st_mode)):
        raise ValueError("unexpected apt directory")
    candidates = []
    main = apt_dir / "sources.list"
    if main.exists() or main.is_symlink():
        candidates.append(main)
    parts = apt_dir / "sources.list.d"
    if parts.exists() or parts.is_symlink():
        if not stat.S_ISDIR(parts.lstat().st_mode):
            raise ValueError("unexpected source directory type")
        candidates.extend(sorted(path for path in parts.iterdir() if path.suffix in (".list", ".sources")))
    plans = []
    isolated = 0
    for path in candidates:
        info = regular_file(path)
        with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), "rb") as source:
            if fingerprint(os.fstat(source.fileno())) != fingerprint(info):
                raise ValueError("source changed during validation")
            original = source.read()
        transform = isolate_sources if path.suffix == ".sources" else isolate_list
        updated, count = transform(original.decode("utf-8"))
        isolated += count
        if updated.encode("utf-8") != original:
            plans.append((path, info, original, updated.encode("utf-8")))
    # Hold and revalidate every edited file before the first write.
    with contextlib.ExitStack() as stack:
        writes = []
        for path, info, original, updated in plans:
            if path.parent.resolve() != path.parent or fingerprint(regular_file(path)) != fingerprint(info):
                raise ValueError("source changed during validation")
            source = stack.enter_context(os.fdopen(os.open(path, os.O_RDWR | os.O_NOFOLLOW), "r+b"))
            if fingerprint(os.fstat(source.fileno())) != fingerprint(info) or source.read() != original:
                raise ValueError("source changed during validation")
            writes.append((source, updated))
        for source, updated in writes:
            source.seek(0)
            source.write(updated)
            source.truncate()
    return len(candidates), len(plans), isolated


def main():
    try:
        scanned, edited, isolated = isolate(Path("/etc/apt"))
    except (OSError, UnicodeError, ValueError):
        print("Geometry apt source isolation failed; no source contents logged.", file=sys.stderr)
        return 1
    print(f"Geometry apt sources: scanned={scanned} edited={edited} isolated_entries={isolated}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
