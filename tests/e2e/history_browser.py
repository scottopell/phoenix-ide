#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx", "httpx-sse"]
# ///
"""Qualification only; never builds or modifies the app checkout.

APP_ROOT must be a separate clean checkout at PIN. APP_BUILD_RECEIPT must name
an independently prepared build receipt: {source_sha, binary_sha256,
ui_files: {relative/path: sha256}}. ui_files covers every ui/dist file except
.gitkeep. Hashes bind the supplied build to this run; source provenance remains
the build owner's attestation, not something a binary hash can prove.

Run: APP_ROOT=... APP_BUILD_RECEIPT=... uv run tests/e2e/history_browser.py
     --output /absolute/new/evidence-directory [--timeout 600]
Requires existing pinned-app Node/Vite/Playwright deps and installed Chromium.
No dependency installation, Cargo invocation, production DB or external server.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from unittest.mock import patch
from urllib.parse import urlencode

PIN = "adeb9c5dbd6213a7599d0fa026d24228d3e039eb"
ROOT = Path(__file__).resolve().parents[2]
FAST = "[[perf:1]] [[ttft:0]]"


def digest(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True,
                                   timeout=10).strip()


def ui_hashes(app):
    dist = app / "ui/dist"
    files = sorted(p for p in dist.rglob("*") if p.is_file() and p.name != ".gitkeep")
    if not files or not (dist / "index.html").is_file():
        raise ValueError("matching built ui/dist/index.html required")
    if any(p.is_symlink() for p in dist.rglob("*")):
        raise ValueError("UI build must not contain symlinks")
    return {str(p.relative_to(dist)): digest(p) for p in files}


def identity(app, receipt_path):
    app = app.resolve(strict=True)
    if app == ROOT.resolve() or git(app, "rev-parse", "--show-toplevel") != str(app):
        raise ValueError("APP_ROOT must be an independent checkout root")
    if git(app, "rev-parse", "HEAD") != PIN:
        raise ValueError("APP_ROOT full SHA differs from frozen app PIN")
    if git(app, "status", "--porcelain", "--untracked-files=no"):
        raise ValueError("APP_ROOT tracked source is dirty")
    binary = app / "target/debug/phoenix_ide"
    if not binary.is_file() or binary.resolve() != binary or not os.access(binary, os.X_OK):
        raise ValueError("real standalone executable target/debug/phoenix_ide required")
    receipt = json.loads(receipt_path.read_text())
    measured = {"source_sha": PIN, "binary_sha256": digest(binary), "ui_files": ui_hashes(app)}
    if any(receipt.get(k) != v for k, v in measured.items()):
        raise ValueError("build receipt source/binary/UI identity mismatch")
    return measured | {"app_root": str(app), "app_tree": git(app, "rev-parse", "HEAD^{tree}"),
                       "receipt_sha256": digest(receipt_path),
                       "harness_sha": git(ROOT, "rev-parse", "HEAD"),
                       "harness_files": {str(p.relative_to(ROOT)): digest(p) for p in
                           [Path(__file__), ROOT / "ui/scripts/qualify-history-realserver.mjs"]},
                       "source_provenance": "independent build receipt attestation"}


def load_app_harness(app):
    spec = importlib.util.spec_from_file_location("pinned_history_e2e", app / "tests/e2e/run.py")
    module = importlib.util.module_from_spec(spec)
    with patch.object(sys, "dont_write_bytecode", True):
        spec.loader.exec_module(module)
    if module.ROOT.resolve() != app or module.BINARY != app / "target/debug/phoenix_ide":
        raise ValueError("pinned harness resolves another app/binary")
    return module


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def append(path, value):
    with path.open("a") as stream:
        stream.write(json.dumps(value, sort_keys=True) + "\n")


def page_keys(snapshot):
    return [(s["segment_ordinal"], m["sequence_id"], m["message_id"])
            for s in snapshot["segments"] for m in s["messages"]]


def validate_pages(pages):
    """Pages arrive newest first; reject repetition, missing progress, bad order."""
    seen = set()
    keys = []
    for page in reversed(pages):
        batch = page_keys(page)
        if batch != sorted(batch):
            raise AssertionError("page is not chronologically ordered")
        for key in batch:
            if key in seen:
                raise AssertionError("aggregate paging repeated a message occurrence")
            seen.add(key)
            keys.append(key)
    if keys != sorted(keys):
        raise AssertionError("cross-page chronological ordering violated")
    return keys


def complete_occurrences(pages, members):
    omitted = {b["represented_successor_opening_message_id"] for b in pages[0]["boundaries"]
               if b["represented_successor_opening_message_id"] is not None}
    expected = [(ordinal, m["sequence_id"], m["message_id"])
                for ordinal, messages in enumerate(members) for m in messages
                if m["message_id"] not in omitted]
    actual = validate_pages(pages)
    assert actual == expected, "aggregate paging differs from authoritative member occurrences"
    return actual


class Fixture:
    def __init__(self, harness, base, output):
        import httpx
        self.h, self.base, self.out = harness, base, output
        self.client = httpx.Client(base_url=base, timeout=10, trust_env=False)

    def request(self, method, path, body=None):
        response = self.client.request(method, path, json=body)
        append(self.out / "setup-api.jsonl", {"method": method, "path": path,
            "status": response.status_code, "request": body, "response": response.text})
        response.raise_for_status()
        return response.json()

    def wait(self, row, exhausted=False, message_id=None):
        predicate = self.h._context_exhausted_event if exhausted else self.h._terminal_event
        def observed(kind, raw):
            append(self.out / "setup-sse.jsonl", {"row": row, "event": kind,
                "data": raw[:8000], "truncated": len(raw) > 8000,
                "sha256": hashlib.sha256(raw.encode()).hexdigest()})
            return predicate(kind, raw)
        self.h._wait_for_sse_signal(self.base, row, observed, 20)
        expected = "context_exhausted" if exhausted else "idle"
        deadline = time.monotonic() + 20
        while True:
            data = self.request("GET", f"/api/conversations/{row}")
            current = self.h._state_str(data["conversation"]["state"])
            evidence = message_id is None or (any(m["message_id"] == message_id for m in data["messages"])
                and (exhausted or self.h._has_agent_after_message(data["messages"], message_id)))
            if current == expected and evidence:
                return data
            assert current != "error", data["conversation"]
            if time.monotonic() >= deadline:
                raise TimeoutError(f"SSE observed but persisted state did not settle: {row}")
            time.sleep(.05)

    def send(self, row, text, exhausted=False):
        mid = str(uuid.uuid4())
        accepted = self.request("POST", f"/api/conversations/{row}/chat",
                                {"text": text, "images": [], "files": [], "message_id": mid})
        assert accepted.get("queued") is True and accepted.get("steering") is not True, accepted
        canonical_mid = f"{row}:{mid}"
        data = self.wait(row, exhausted, canonical_mid)
        assert any(m["message_id"] == canonical_mid for m in data["messages"]), "send not persisted"
        if not exhausted:
            assert self.h._has_agent_after_message(data["messages"], canonical_mid), "no agent after accepted send"
        return data

    def continued(self, row):
        self.send(row, "[[scenario:context_window_exceeded]] [[ttft:0]]", True)
        response = self.request("POST", f"/api/conversations/{row}/continue",
            {"handoff": FAST + " qualification continuation", "message_id": str(uuid.uuid4())})
        successor = response["conversation_id"]
        assert response["status"] == "accepted" and successor != row
        self.wait(successor)
        before = self.request("GET", f"/api/conversations/{row}")
        assert before["conversation"]["continued_in_conv_id"] == successor
        return successor

    def build(self, kind, cwd):
        if kind == "ordinary":
            created = self.request("POST", "/api/product-conversations/new", {
                "cwd": str(cwd), "model": self.h._default_model(self.base),
                "objective": FAST + " qualification ordinary", "images": [], "request_id": str(uuid.uuid4())})
            row = created["transcript_row_id"]
            route = created["canonical_route"]
            endpoint = f'/api/product-conversations/{created["product_conversation_id"]}'
            self.wait(row)
        else:
            row = self.request("POST", "/api/global/coordinator")["conversation"]["id"]
            route = f"/global/{row}"
            endpoint = f"/api/global/coordinator/history/{row}"
        setting = (endpoint + "/automatic-continuation" if kind == "ordinary"
                   else "/api/global/coordinator/automatic-continuation")
        disabled = self.request("PUT", setting, {"auto_continue_on_context_exhaustion": False})
        assert disabled["auto_continue_on_context_exhaustion"] is False
        members, old, tool = [], None, None
        persisted = 0
        member_messages = []
        for chapter in range(3):
            members.append(row)
            for turn in range(36):
                text = ("[[scenario:think]] [[ttft:0]]" if chapter == 0 and turn == 1 else FAST)
                data = self.send(row, text + f" {kind}-{chapter}-{turn}")
                if old is None:
                    old = next(m["message_id"] for m in data["messages"] if m["message_type"] == "user")
                if tool is None:
                    for m in data["messages"]:
                        for block in m["content"] if isinstance(m["content"], list) else []:
                            if block.get("type") == "tool_use" and block.get("name") == "think":
                                tool = {"message_id": m["message_id"], "tool_id": block["id"]}
            if chapter != 2:
                row = self.continued(row)
            messages = self.request("GET", f"/api/conversations/{members[-1]}")["messages"]
            member_messages.append(messages)
            persisted += len(messages)
        assert persisted > 201 and len(set(members)) >= 3 and tool is not None
        pages, cursor, cursors = [], None, set()
        for _ in range(8):
            query = {"message_limit": 100, "boundary_representation": "original_messages"}
            if cursor:
                query["before"] = cursor
            page = self.request("GET", endpoint + "?" + urlencode(query))
            pages.append(page)
            if not page["has_older"]:
                break
            cursor = page["before"]
            assert cursor and cursor not in cursors, "cursor did not advance"
            cursors.add(cursor)
        else:
            raise AssertionError("paging exceeded eight pages")
        keys = complete_occurrences(pages, member_messages)
        assert len(keys) > 201 and len(pages) >= 3
        assert pages[0]["latest_transcript_row_id"] == row
        assert len(pages[0]["boundaries"]) >= 2
        if kind == "global":
            route = f"/global/{row}"
        return {"kind": kind, "route": route, "endpoint": endpoint, "members": members,
                "old_message": old, "tool": tool, "persisted_rows": persisted,
                "page_count": len(pages), "occurrences": keys,
                "boundaries": pages[0]["boundaries"]}


def stop(proc):
    # Descendants may outlive an already-exited driver (e.g. its wall bound).
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    proc.wait(timeout=5)


@contextlib.contextmanager
def owned_process(command, cwd, env, log):
    with log.open("w") as stream:
        proc = subprocess.Popen(command, cwd=cwd, env=env, stdout=stream,
                                stderr=subprocess.STDOUT, start_new_session=True)
        try:
            yield proc
        finally:
            stop(proc)


def qualify(app, output, timeout):
    h = load_app_harness(app)
    # Import only helpers; never invoke run.py main/build/test suite. Retain startup
    # logs even if _server removes its temp directory after startup failure.
    original_env, original_start = h._server_env, h._start_server_with_retries
    safe = {k: os.environ[k] for k in ("PATH", "SYSTEMROOT", "TMPDIR") if k in os.environ}
    def isolated(tmpdir, parent_env=None):
        env = original_env(tmpdir, safe)
        assert env["PHOENIX_BIND_ADDR"] == "127.0.0.1" and env["DEFAULT_MODEL"] == "mock"
        write_json(output / "isolation.json", {k: env[k] for k in (
            "HOME", "PHOENIX_DB_PATH", "PHOENIX_DATA_DIR", "PHOENIX_BIND_ADDR", "DEFAULT_MODEL")})
        return env
    def retained_start(env, tmpdir):
        started, children = False, []
        popen = subprocess.Popen
        def tracked(*args, **kwargs):
            proc = popen(*args, **kwargs)
            children.append(proc)
            return proc
        try:
            with patch.object(h.subprocess, "Popen", tracked):
                value = original_start(env, tmpdir)
            started = True
            return value
        finally:
            if not started:
                for proc in children:
                    if proc.poll() is None:
                        proc.terminate()
                        try:
                            proc.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            proc.kill()
                            proc.wait(timeout=5)
            for path in tmpdir.glob("phoenix-startup-*.log"):
                shutil.copyfile(path, output / path.name)
    with patch.object(h, "_server_env", isolated), patch.object(h, "_start_server_with_retries", retained_start), \
         patch.object(h, "_profile_dir_from_env", return_value=None):
        with h._server() as (base, log, pid, _), tempfile.TemporaryDirectory(prefix="history-browser-") as temp:
            temp = Path(temp)
            try:
                write_json(output / "server.json", {"base_url": base, "pid": pid})
                fixture = Fixture(h, base, output)
                try:
                    work = temp / "cwd"
                    work.mkdir()
                    aggregates = [fixture.build(kind, work) for kind in ("ordinary", "global")]
                finally:
                    fixture.client.close()
                write_json(output / "fixture.json", {"aggregates": aggregates})
                port = h._free_port()
                ui = f"http://127.0.0.1:{port}"
                config = temp / "preview.mjs"
                config.write_text("export default " + json.dumps({"root": str(app / "ui"),
                    "build": {"outDir": str(app / "ui/dist")}, "preview": {"host": "127.0.0.1",
                    "port": port, "strictPort": True, "proxy": {"/api": {"target": base, "xfwd": True},
                    "/preview": {"target": base}}}}) + ";\n")
                # A separate preview serves the hashed dist, not the debug backend's
                # cwd-relative asset fallback (the backend cwd is its temporary DB root).
                node_env = safe | {"HOME": str(temp), "APP_ROOT": str(app),
                    "HISTORY_BASE_URL": ui, "HISTORY_API_URL": base,
                    "HISTORY_OUTPUT": str(output), "HISTORY_FIXTURE": str(output / "fixture.json")}
                cache = os.environ.get("PLAYWRIGHT_BROWSERS_PATH")
                if not cache:
                    cache = str(Path.home() / ("Library/Caches/ms-playwright" if sys.platform == "darwin"
                                              else ".cache/ms-playwright"))
                node_env["PLAYWRIGHT_BROWSERS_PATH"] = str(Path(cache).resolve())
                with owned_process(["node", str(app / "ui/node_modules/vite/bin/vite.js"),
                                    "preview", "--config", str(config)], temp, node_env,
                                   output / "preview.log") as preview:
                    import httpx
                    deadline = time.monotonic() + 30
                    while True:
                        assert preview.poll() is None, "preview exited; see preview.log"
                        try:
                            response = httpx.get(ui, timeout=1, trust_env=False)
                            if response.status_code == 200:
                                assert hashlib.sha256(response.content).hexdigest() == digest(app / "ui/dist/index.html")
                                break
                        except httpx.TransportError:
                            pass
                        if time.monotonic() >= deadline:
                            raise TimeoutError("matching built UI preview readiness")
                        time.sleep(.1)
                    with owned_process(["node", str(ROOT / "ui/scripts/qualify-history-realserver.mjs")],
                            temp, node_env, output / "browser.log") as driver:
                        driver.wait(timeout=timeout)
                        assert driver.returncode == 0, "browser assertions failed; see browser receipts/log"
            finally:
                shutil.copyfile(log, output / "backend.log")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--output", type=Path)
    mode.add_argument("--build-receipt", type=Path,
                      help="bind an already-built pinned checkout to artifact hashes; never builds")
    parser.add_argument("--timeout", type=int, default=600, help="whole-run wall bound, 120..1200 seconds")
    args = parser.parse_args()
    if not 120 <= args.timeout <= 1200:
        parser.error("timeout must be between 120 and 1200 seconds")
    if args.build_receipt:
        app = Path(os.environ["APP_ROOT"]).resolve(strict=True)
        if git(app, "rev-parse", "HEAD") != PIN or git(app, "status", "--porcelain", "--untracked-files=no"):
            raise ValueError("receipt requires clean frozen app source")
        binary = app / "target/debug/phoenix_ide"
        receipt = {"source_sha": PIN, "source_tree": git(app, "rev-parse", "HEAD^{tree}"),
                   "binary_sha256": digest(binary), "ui_files": ui_hashes(app),
                   "harness_sha": git(ROOT, "rev-parse", "HEAD"),
                   "cargo_lock_sha256": digest(app / "Cargo.lock"),
                   "ui_lock_sha256": digest(app / "ui/pnpm-lock.yaml"),
                   "toolchains": {name: subprocess.check_output([name, "--version"], cwd=app, text=True, timeout=10).strip()
                                  for name in ("rustc", "cargo", "node", "pnpm")}}
        write_json(args.build_receipt.resolve(), receipt)
        identity(app, args.build_receipt.resolve())
        return
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    result = {"status": "failed", "app_pin": PIN}
    def expired(*_):
        raise TimeoutError("qualification whole-run wall-clock limit")
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(args.timeout)
    started = time.monotonic()
    try:
        app = Path(os.environ["APP_ROOT"]).resolve()
        receipt = Path(os.environ["APP_BUILD_RECEIPT"]).resolve()
        before = identity(app, receipt)
        write_json(output / "identity.json", before)
        qualify(app, output, args.timeout)
        assert identity(app, receipt) == before, "app/artifact identity changed during qualification"
        browser = json.loads((output / "browser-result.json").read_text())
        assert browser["status"] == "passed_assertions" and browser["checks"]
        result.update(status="passed_assertions", checks=browser["checks"], uncovered=browser["uncovered"])
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        signal.alarm(0)
        result["elapsed_seconds"] = time.monotonic() - started
        write_json(output / "result.json", result)


if __name__ == "__main__":
    main()
