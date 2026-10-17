#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["httpx", "httpx-sse", "opentelemetry-proto==1.39.0"]
# ///
"""One artifact-reused native BFCache probe; never shipping or overhead acceptance."""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
from urllib.parse import urlencode, quote
import history_opening_browser as opening


def deadline(signum, frame):
    raise TimeoutError("bounded native probe lifetime exhausted")


def main():
    signal.signal(signal.SIGTERM, deadline)
    signal.signal(signal.SIGALRM, deadline)
    signal.alarm(180)
    app = Path(os.environ["APP_ROOT"]).resolve()
    artifact = Path(os.environ["PROBE_ARTIFACT"]).resolve()
    out = Path(os.environ["PROBE_OUTPUT"]).resolve()
    out.mkdir(parents=True, exist_ok=True)
    s = opening.shipping
    receipt = json.loads((artifact / "opening-instrumented-receipt.json").read_text())
    assert receipt["app_source"] == opening.PIN and receipt["app_tree"] == opening.TREE_PIN
    assert receipt["harness_source"] == "1e0423216453f7487a3d3e4ac5f3620303a33b82"
    assert s.git(app, "rev-parse", "HEAD") == opening.PIN
    assert s.git(app, "rev-parse", "HEAD^{tree}") == opening.TREE_PIN
    exe = artifact / "opening-instrumented-libtest"
    assert s.digest(exe) == receipt["executable_sha256"]
    exe.chmod(0o700)
    binding = receipt["ui_artifacts"]["controlled_functional"]
    owner = json.loads((artifact / "opening-ui-build-owner.json").read_text())
    assert owner["ui_artifacts"]["controlled_functional"] == binding
    manifest = artifact / "pinned-app/ui/controlled-functional-build.json"
    assert s.digest(manifest) == binding["manifest_sha256"]
    ui = artifact / "pinned-app/ui/dist-controlled-functional"
    placeholder = "ui/dist-controlled-functional/.gitkeep"
    assert binding["ui_files"][placeholder] == hashlib.sha256(b"").hexdigest()
    (ui / ".gitkeep").write_bytes(b"")
    assert {str(p.relative_to(artifact / "pinned-app")): s.digest(p) for p in ui.rglob("*") if p.is_file()} == binding["ui_files"]
    assert s.digest(app / "ui/pnpm-lock.yaml") == receipt["ui_lock_sha256"]
    s.write_json(out / "provenance.json", {
        "scope": "separate_native_capability_not_stock_overhead_or_shipping",
        "artifact": 12093422759, "app_source": opening.PIN,
        "app_tree": opening.TREE_PIN, "executable_sha256": s.digest(exe),
        "manifest_sha256": s.digest(manifest), "fixture": "disposable_recreated_recipe",
        "sample_rate": 1, "full_acceptance": False, "overhead_clearance": False})
    h = s.load_app_harness(app)
    failures = opening.LifecycleFailures()
    with tempfile.TemporaryDirectory(prefix="bfc-", dir="/tmp") as owned:
        work = Path(owned)
        work.chmod(0o700)
        control = work / "c.sock"
        port = h._free_port()
        base = f"http://127.0.0.1:{port}"
        env = {"PATH": os.environ["PATH"], "HOME": str(work), "TMPDIR": str(work),
               "PHOENIX_PORT": str(port), "PHOENIX_TEST_OPENING_CONTROL": str(control)}
        telemetry = opening.load_telemetry_modules(app)
        with telemetry[0].telemetry_sink(out, "bfcache-once", lifetime_seconds=260) as receiver:
            env = opening.telemetry_environment(env, receiver)
            command = [str(exe), opening.DRIVER, "--exact", "--ignored", "--test-threads=1", "--nocapture"]
            with s.owned_process(command, work, env, out / "private-runtime.log") as server:
                startup = None
                try:
                    opening.readiness(server, base + "/api/auth/status")
                    startup = opening.startup_identity(out / "private-runtime.log", exe, work, port)
                    fixture = s.Fixture(h, base, out)
                    try:
                        opening.authenticate_fixture(fixture.client, opening.owned_qualification_password(startup, work / "credential.json"))
                        fixture.h = opening.AuthenticatedHarness(h, fixture.client)
                        fixture.completion = opening.OwnedFixtureCompletion(startup, fixture)
                        cwd = work / "cwd"
                        cwd.mkdir(mode=0o700)
                        aggregate = opening.equal_text_fixture(fixture, fixture.build("ordinary", cwd), writable=True)
                    finally:
                        fixture.client.close()
                    source = aggregate["members"][0]
                    mid = aggregate["old_message"]
                    route = aggregate["route"] + "?" + urlencode({"source_transcript": source}) + "#message-" + quote(mid, safe="")
                    input_path = work / "input.json"
                    s.write_json(input_path, {"id": mid, "source": source, "route": route})
                    preview_port = h._free_port()
                    fault = work / "fault.json"
                    s.write_json(fault, {"aggregatePath": aggregate["endpoint"], "failRoster": False, "streamPath": "", "drop": 0})
                    scripts = Path(__file__).resolve().parents[2] / "ui/scripts"
                    transport = scripts / "qualify-history-opening-realserver.mjs"
                    config = work / "preview.mjs"
                    config.write_text("import {previewTransport} from " + json.dumps(transport.as_uri()) + ";const c=" + json.dumps({"root": str(app / "ui"), "build": {"outDir": str(ui)}, "preview": {"host": "127.0.0.1", "port": preview_port, "strictPort": True}}) + ";c.preview.proxy=previewTransport(" + json.dumps(base) + "," + json.dumps(str(fault)) + "," + json.dumps(str(work / "transport.jsonl")) + ");export default c;")
                    node_env = os.environ | {"HOME": str(work), "HISTORY_BASE_URL": f"http://127.0.0.1:{preview_port}", "HISTORY_AUTH_HOME": startup["home"], "HISTORY_AUTH_CREDENTIAL_FILE": startup["credential_file"], "PROBE_INPUT": str(input_path)}
                    with s.owned_process(["node", str(app / "ui/node_modules/vite/bin/vite.js"), "preview", "--config", str(config)], work, node_env, out / "private-preview.log") as preview:
                        opening.readiness(preview, node_env["HISTORY_BASE_URL"])
                        completed = subprocess.run(["node", str(scripts / "history-bfcache-probe.mjs")], env=node_env, cwd=work, timeout=80)
                        s.write_json(out / "driver-exit.json", {"returncode": completed.returncode, "acceptance": False})
                finally:
                    signal.alarm(85)
                    if startup:
                        opening.normal_server_completion(server, out, startup, control, failures, request_stop=True)
    signal.alarm(0)
    s.write_json(out / "completion.json", {"server": "normal", "receiver": "closed", "full_acceptance": False})
    if completed.returncode:
        raise RuntimeError("probe prerequisites failed; inspect safe receipt")


if __name__ == "__main__":
    main()
