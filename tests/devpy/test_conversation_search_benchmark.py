import importlib.util
import json
import hashlib
import os
import sqlite3
import subprocess
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest import mock
ROOT = Path(__file__).parents[2]
spec = importlib.util.spec_from_file_location("conversation_search_benchmark", ROOT / "scripts/conversation_search_benchmark.py")
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


class ConversationSearchBenchmarkTests(unittest.TestCase):
    def setUp(self):
        clean = {key:value for key,value in os.environ.items() if not key.startswith(("CARGO", "CC_")) and key not in {"CC","HOST_CC","TARGET_CC","CROSS_COMPILE","RUSTC","RUSTC_WRAPPER","RUSTC_WORKSPACE_WRAPPER","LIBSQLITE3_SYS_USE_PKG_CONFIG"}}
        patch = mock.patch.dict(os.environ, clean, clear=True)
        patch.start()
        self.addCleanup(patch.stop)

    @staticmethod
    def _complete_run(**overrides):
        samples = [
            {"case_id": "case", "surface": "tool", "phase": "first_use_fresh_pool_os_cache_uncontrolled", "iteration": 0,
             "ok": True, "result_count":1, "result_identity":["id"], "result":"one", "result_digest":hashlib.sha256(b"one").hexdigest(), "duration_ms": 100},
            {"case_id": "case", "surface": "tool", "phase": "warmup_discarded", "iteration": 0,
             "ok": True, "result_count":1, "result_identity":["id"], "result":"one", "result_digest":hashlib.sha256(b"one").hexdigest(), "duration_ms": 100},
            *[
                {"case_id": "case", "surface": "tool", "phase": "warm", "iteration": i,
                 "ok": True, "result_count":1, "result_identity":["id"], "result":"one", "result_digest":hashlib.sha256(b"one").hexdigest(), "duration_ms": i + 1}
                for i in range(10)
            ],
        ]
        value = {
            "fixture_sha256": "a", "schema_digest": "schema", "migration_ledger": [],
            "scenario_digest": "s", "profile": "release",
            "warmup_runs": 1, "measured_warm_runs": 10, "commit": "deadbeef",
            "environment":{"host":"host","platform":"test","processor":"test","cpu_count":"2"}, "sqlite_pragmas":{"sqlite_version":"test","journal_mode":"wal","synchronous":2,"busy_timeout_ms":300000,"foreign_keys":True,"query_only":False},
            "runtime": {"worker_threads":2,"measurement_clock":"monotonic"}, "explain_enabled": False,
            "build_configuration": {"rustc_version_verbose": "rustc", "cargo_version": "cargo", "target": "host", "profile": "release", "features": [], "environment": {}, "cargo_config_hashes":{}, "release_profile":{}, "native_compiler":{"path":"cc","sha256":"a"*64,"version":"version"}},
            "expected_case_surface_set": [["case", "tool"]],
            "case_policies": [{"case_id":"case","surface":"tool","policy":{"limit":20,"scope":"Global","visibility":"All","grouping":"None","match_mode":"FinalTokenPrefix","lexical_expression":"x"}}],
            "measurement_digest":"harness", "run_uuid": __import__("uuid").uuid4().hex, "started_at_unix":float(__import__("time").time_ns()), "completed_at_unix":float(__import__("time").time_ns()),
            "explain_plans": [], "samples": samples,
            "fixture_validation":{"transcript_count":1,"freshness_batch_size":64,"locator_orphans":0,"missing_physical_rows":0,"unlocated_physical_rows":0}, "tool_oracle_regime":"none historical", "measurement_regimes":["first_use_fresh_pool_os_cache_uncontrolled", "warm"],
        }
        value.update(overrides)
        return value

    def test_staged_files_require_root_ignore(self):
        with tempfile.TemporaryDirectory() as d:
            repo = Path(d).resolve()
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            (repo / ".gitignore").write_text("private/\n")
            with mock.patch.object(bench, "__file__", str(repo / "scripts" / "helper.py")):
                bench._ensure_ignored_artifacts(repo / "private")
                (repo / ".gitignore").write_text("private/captured.db\nprivate/capture-manifest.json\nprivate/scenarios.json\nprivate/report.md\nprivate/runs/\nprivate/__private_staged_probe__\n")
                with self.assertRaisesRegex(SystemExit, "unignored"):
                    bench._ensure_ignored_artifacts(repo / "private")

    def test_exact_freshness_and_target_flags(self):
        for evidence in [{"index_fresh": True}, {**self._complete_run()["fixture_validation"], "locator_orphans":1}]:
            with self.assertRaisesRegex(SystemExit, "freshness"):
                bench._validate_run(self._complete_run(fixture_validation=evidence), "bad")
        with mock.patch.dict("os.environ", {"CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS":"-C target-cpu=native", "CARGO_REGISTRY_TOKEN":"secret"}), mock.patch.object(bench.subprocess, "check_output", return_value="version"):
            env = bench._build_configuration()["environment"]
            self.assertIn("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS", env)
            self.assertNotIn("CARGO_REGISTRY_TOKEN", env)

    def test_label_reservation_refuses_second_writer(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "capture-manifest.json").write_text(json.dumps({"kind":"conversation-search-fixture"}))
            args = type("Args", (), {"artifacts":str(root), "label":"same","timeout":10})()
            def nested(_):
                with self.assertRaisesRegex(SystemExit, "reserved"):
                    bench.run(type("Args", (), {"artifacts":str(root), "label":"different","timeout":10})())
                return 0
            with mock.patch.object(bench,"_validate_before_run"), mock.patch.object(bench, "_run_reserved", side_effect=nested):
                self.assertEqual(bench.run(args), 0)
            self.assertFalse((root / "runs" / ".active-run.reserved").exists())

    def test_capture_rejects_foreign_directory_without_chmod(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d); (root / "foreign").write_text("keep"); root.chmod(0o755)
            args = type("Args", (), {"artifacts":str(root)})()
            with self.assertRaisesRegex(SystemExit, "unrecognized"): bench.snapshot(args)
            self.assertEqual(root.stat().st_mode & 0o777, 0o755)
            self.assertFalse((root / ".capture-lock").exists())

    def test_external_process_capture_lock_is_respected(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d); lock = root / ".capture-lock"
            code = "import fcntl,sys,time; f=open(sys.argv[1],'a'); fcntl.flock(f,fcntl.LOCK_EX); print('ready',flush=True); time.sleep(30)"
            child = subprocess.Popen([__import__("sys").executable, "-c", code, str(lock)], stdout=subprocess.PIPE, text=True)
            try:
                self.assertEqual(child.stdout.readline().strip(), "ready")
                args = type("Args", (), {"artifacts":str(root)})()
                with self.assertRaisesRegex(SystemExit, "(?:already active|unrecognized|empty)"): bench.snapshot(args)
            finally:
                child.terminate(); child.wait(timeout=5); child.stdout.close()

    def test_native_flags_recorded_without_secret_registry_values(self):
        with mock.patch.dict("os.environ", {"CC":"clang", "CFLAGS_aarch64_apple_darwin":"-O2", "LIBSQLITE3_FLAGS":"SQLITE_DEFAULT_CACHE_SIZE=-8000",  "CARGO_REGISTRY_TOKEN":"secret"}), mock.patch.object(bench.subprocess,"check_output",return_value="host: test"):
            env=bench._build_configuration()["environment"]
            self.assertEqual(env["CC"],"clang")
            self.assertEqual(env["LIBSQLITE3_FLAGS"],"SQLITE_DEFAULT_CACHE_SIZE=-8000")
            self.assertEqual(env["CFLAGS_aarch64_apple_darwin"],"-O2")
            self.assertNotIn("CARGO_REGISTRY_TOKEN",env)

    def test_invalid_marker_does_not_chmod_directory(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d); (root/"capture-manifest.json").write_text("{}")
            root.chmod(0o755)
            with self.assertRaisesRegex(SystemExit,"ownership"):
                bench.snapshot(type("Args",(),{"artifacts":str(root)})())
            self.assertEqual(root.stat().st_mode & 0o777,0o755)

    def test_incomplete_capture_is_preserved_and_refused(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d); root.chmod(0o755); (root/".capture-pending").write_text("unknown owner")
            partial=root/".captured.db.123.tmp"; partial.write_bytes(b"partial")
            src=root.parent/"unused-source"; src.write_bytes(b"source")
            try:
                with self.assertRaisesRegex(SystemExit,"incomplete prior capture"):
                    bench.snapshot(type("Args",(),{"artifacts":str(root),"source":str(src)})())
                self.assertEqual(partial.read_bytes(),b"partial")
                self.assertEqual(root.stat().st_mode & 0o777, 0o755)
                self.assertFalse((root/".capture-lock").exists())
            finally: src.unlink()

    def test_result_identity_and_config_evidence_are_required(self):
        run=self._complete_run(); del run["build_configuration"]["cargo_config_hashes"]
        with self.assertRaisesRegex(SystemExit,"build configuration"): bench._validate_run(run,"bad")
        run=self._complete_run(); run["samples"][0]["result_identity"]=["changed"]
        with self.assertRaisesRegex(SystemExit,"output mismatch"): bench._validate_run(run,"bad")

    def test_unsafe_lock_and_invalid_samples_are_refused(self):
        for duration in [-1, True, float("nan"), float("inf")]:
            run=self._complete_run();run["samples"][0]["duration_ms"]=duration
            with self.assertRaisesRegex(SystemExit,"duration"):bench._validate_run(run,"bad")
        run=self._complete_run(case_policies=[{"garbage":1}])
        with self.assertRaisesRegex(SystemExit,"policies"):bench._validate_run(run,"bad")
        with tempfile.TemporaryDirectory() as d:
            parent=Path(d);root=parent/"artifact";root.mkdir();target=parent/"foreign";target.write_text("keep");target.chmod(0o644)
            (root/".capture-lock").symlink_to(target)
            with self.assertRaises((OSError,SystemExit)):bench.snapshot(type("Args",(),{"artifacts":str(root)})())
            self.assertEqual(target.stat().st_mode&0o777,0o644)

    def test_wal_capture_preserves_source_files_and_committed_rows(self):
        with tempfile.TemporaryDirectory() as d:
            parent=Path(d);source=parent/"source.db";conn=sqlite3.connect(source)
            conn.execute("pragma journal_mode=wal");conn.execute("create table x(v)");conn.execute("insert into x values(42)");conn.commit()
            initial={p.name for p in parent.iterdir()}
            reader=sqlite3.connect(bench._uri(source),uri=True);backup=sqlite3.connect(":memory:");reader.backup(backup)
            self.assertEqual(backup.execute("select v from x").fetchall(),[(42,)])
            reader.close();backup.close();self.assertEqual({p.name for p in parent.iterdir()},initial);conn.close()
            initial={p.name for p in parent.iterdir()};artifact=parent/"artifact"
            with self.assertRaisesRegex(SystemExit,"offline"):
                bench.snapshot(type("Args",(),{"artifacts":str(artifact),"source":str(source),"force":False,"offline_snapshot":False,"busy_timeout":1,"deadline":10,"retries":1})())
            self.assertEqual({p.name for p in parent.iterdir()if p!=artifact},initial)

    def test_intervals_policies_and_harness_digest_are_checked(self):
        for start,end in [(3,2),(float("nan"),4),(True,4)]:
            run=self._complete_run(started_at_unix=start,completed_at_unix=end)
            with self.assertRaisesRegex(SystemExit,"interval"):bench._validate_run(run,"bad")
        run=self._complete_run();run["case_policies"][0]["policy"]["grouping"]="unknown"
        with self.assertRaisesRegex(SystemExit,"policies"):bench._validate_run(run,"bad")
        self.assertEqual(len(bench._measurement_digest()),64)

    def test_live_source_and_missing_attestation_are_refused_before_open(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d); source=root/"source.db"; sqlite3.connect(source).close()
            args=type("Args",(),{"source":str(source),"artifacts":str(root/"fixture"),"offline_snapshot":False})()
            with mock.patch.object(bench.sqlite3,"connect") as opened:
                with self.assertRaisesRegex(SystemExit,"attestation"): bench.snapshot(args)
                opened.assert_not_called()
            args.artifacts=str(root/"fixture2");args.offline_snapshot=True;Path(str(source)+"-wal").write_bytes(b"live")
            with mock.patch.object(bench.sqlite3,"connect") as opened:
                with self.assertRaisesRegex(SystemExit,"standalone"): bench.snapshot(args)
                opened.assert_not_called()

    def test_raw_digest_compiler_and_live_inode_are_checked(self):
        run=self._complete_run();run["samples"][0]["result"]="changed private text"
        with self.assertRaisesRegex(SystemExit,"digest mismatch"):bench._validate_run(run,"bad")
        run=self._complete_run();run["build_configuration"]["rustc_version_verbose"]=""
        with self.assertRaisesRegex(SystemExit,"compiler identity"):bench._validate_run(run,"bad")
        with tempfile.TemporaryDirectory() as d:
            home=Path(d);prod=home/".phoenix-ide"/"prod.db";prod.parent.mkdir();sqlite3.connect(prod).close();alias=home/"alias.db";os.link(prod,alias)
            args=type("Args",(),{"source":str(alias),"artifacts":str(home/"artifact"),"offline_snapshot":True})()
            with mock.patch.object(bench.Path,"home",return_value=home),mock.patch.object(bench.sqlite3,"connect") as opened:
                with self.assertRaisesRegex(SystemExit,"production"):bench.snapshot(args)
                opened.assert_not_called()

    def test_complete_execution_shapes_and_unsupported_linkage(self):
        for key,value in [("environment",{"garbage":"x"}),("runtime",{"worker_threads":2}),("sqlite_pragmas",{"read_only":True})]:
            with self.assertRaisesRegex(SystemExit,"metadata"):bench._validate_run(self._complete_run(**{key:value}),"bad")
        with mock.patch.dict("os.environ",{"LIBSQLITE3_SYS_USE_PKG_CONFIG":""}):
            with self.assertRaisesRegex(SystemExit,"external SQLite linkage unsupported"):bench._build_configuration()
        run=self._complete_run();run["environment"]["cpu_count"]="0"
        with self.assertRaisesRegex(SystemExit,"environment values"):bench._validate_run(run,"bad")

    def test_project_cargo_config_identity_is_relative_and_hash_sensitive(self):
        with tempfile.TemporaryDirectory() as d:
            base=Path(d);recorded=[]
            for name in ("a","b"):
                repo=base/name;(repo/".cargo").mkdir(parents=True);(repo/".cargo/config.toml").write_text("[build]\njobs=2\n")
                with mock.patch.object(bench,"__file__",str(repo/"scripts/helper.py")),mock.patch.object(bench.subprocess,"check_output",return_value="host: test"):
                    recorded.append(bench._build_configuration()["cargo_config_hashes"])
            self.assertEqual(recorded[0],recorded[1])
            self.assertIn("project/.cargo/config.toml",recorded[0])
            (base/"b/.cargo/config.toml").write_text("[build]\njobs=3\n")
            with mock.patch.object(bench,"__file__",str(base/"b/scripts/helper.py")),mock.patch.object(bench.subprocess,"check_output",return_value="host: test"):
                self.assertNotEqual(recorded[0],bench._build_configuration()["cargo_config_hashes"])

    def test_dangling_private_symlink_is_not_followed(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);target=root/"external";link=root/"report.md";link.symlink_to(target)
            with self.assertRaisesRegex(SystemExit,"symlink"):bench._write_private(link,"private")
            self.assertFalse(target.exists())

    def test_report_atomic_writer_preserves_external_hardlink(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);external=root/"foreign";external.write_text("keep");external.chmod(0o644);report=root/"report.md";os.link(external,report)
            bench._write_atomic_private(report,"new report")
            self.assertEqual(external.read_text(),"keep");self.assertEqual(external.stat().st_mode&0o777,0o644)
            self.assertEqual(report.read_text(),"new report")
        with tempfile.TemporaryDirectory() as d:
            raw=Path(d)/"raw.json";raw.write_text("malformed")
            with self.assertRaisesRegex(SystemExit,"decoded"):
                bench._carry_run_metadata(raw,{}, {}, [],run_uuid="id",started_at_unix=1,completed_at_unix=2,measurement_digest="hash")
            self.assertEqual(raw.read_text(),"malformed")

    def test_nonfinite_capture_deadline_is_refused(self):
        for deadline in [float("nan"),float("inf"),-1,0]:
            with tempfile.TemporaryDirectory() as d:
                args=type("Args",(),{"artifacts":str(Path(d)/"artifact"),"source":"unused", "deadline":deadline})()
                with self.assertRaisesRegex(SystemExit,"finite positive"):bench.snapshot(args)

    def test_native_compiler_identity_change_refuses_comparison(self):
        before=self._complete_run();after=self._complete_run()
        after["build_configuration"]["native_compiler"]["sha256"]="b"*64
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);(root/"before.json").write_text(json.dumps(before));(root/"after.json").write_text(json.dumps(after))
            with self.assertRaisesRegex(SystemExit,"measurement regime"):
                bench.compare(type("Args",(),{"before":str(root/"before.json"),"after":str(root/"after.json")})())

    def test_entry_timeouts_and_build_kind_compilers_fail_before_launch(self):
        for timeout in [float("nan"),float("inf"),float("-inf"),0,-1,True]:
            with mock.patch.object(bench.subprocess,"Popen") as launch:
                with self.assertRaisesRegex(SystemExit,"finite positive"):
                    bench.run(type("Args",(),{"timeout":timeout})())
                launch.assert_not_called()
        for key in ["TARGET_CC","HOST_CC","CC_aarch64_apple_darwin","CC_KNOWN_WRAPPER_CUSTOM","CROSS_COMPILE","CRATE_CC_NO_DEFAULTS"]:
            with mock.patch.dict("os.environ",{key:"gcc"}),mock.patch.object(bench.subprocess,"check_output",return_value="host: test"):
                with self.assertRaisesRegex(SystemExit,"native compiler selection unsupported"):bench._build_configuration()

    def test_build_record_schema_matrix_and_effective_selectors(self):
        original=self._complete_run()["build_configuration"]
        for field in original:
            run=self._complete_run();del run["build_configuration"][field]
            with self.assertRaises(SystemExit):bench._validate_run(run,"bad")
            run=self._complete_run();run["build_configuration"][field]=None
            with self.assertRaises(SystemExit):bench._validate_run(run,"bad")
        for key in ["RUSTC","RUSTC_WRAPPER","RUSTC_WORKSPACE_WRAPPER","CARGO_BUILD_RUSTC","CARGO_BUILD_RUSTC_WRAPPER","CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"]:
            with mock.patch.dict("os.environ",{key:"alternate"}):
                with self.assertRaisesRegex(SystemExit,"alternate Cargo compiler"):bench._build_configuration()
        with mock.patch.object(__import__("sys"),"argv",["benchmark","prepare","--force"]):
            with self.assertRaises(SystemExit):bench.main()

    def test_selective_tool_candidate_uses_exact_not_palette_prefix(self):
        conn=sqlite3.connect(":memory:");conn.execute("create virtual table message_fts using fts5(text)")
        conn.execute("insert into message_fts(text) values('abcdef')")
        conn.executemany("insert into message_fts(text) values(?)",[("abcdefgh",)]*1001)
        self.assertEqual(bench._selective_term(conn,"abcdef"),"abcdef")
        self.assertEqual(conn.execute('select count(*) from message_fts where message_fts match ?', ('"abcdef"',)).fetchone()[0],1)
        self.assertEqual(conn.execute('select count(*) from message_fts where message_fts match ?', ('"abcdef"*',)).fetchone()[0],1002)

    def test_constrained_overrides_plans_and_missing_fixture(self):
        for key in ["CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER","CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER","CARGO_INCREMENTAL","CARGO_PROFILE_RELEASE_INCREMENTAL"]:
            with mock.patch.dict("os.environ",{key:"custom"}):
                with self.assertRaisesRegex(SystemExit,"overrides unsupported"):bench._build_configuration()
        with self.assertRaisesRegex(SystemExit,"EXPLAIN"):
            bench._validate_run(self._complete_run(explain_enabled=True,explain_plans=[]),"bad")
        run=self._complete_run(explain_enabled=True)
        run["explain_plans"]=[{"case_id":"case","surface":"tool","query":"q","policy":run["case_policies"][0]["policy"],"plan":["SCAN message_fts"]}]
        bench._validate_run(run,"valid")
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);(root/"capture-manifest.json").write_text(json.dumps({"kind":"conversation-search-fixture"}))
            with self.assertRaisesRegex(SystemExit,"existing fixture"):
                bench.run(type("Args",(),{"timeout":10,"artifacts":str(root),"label":"x"})())
            self.assertFalse((root/"runs").exists())

    def test_cargo_config_runner_is_refused_without_resolution(self):
        with tempfile.TemporaryDirectory() as d:
            repo=Path(d);(repo/".cargo").mkdir();(repo/".cargo/config.toml").write_text('[target.x86_64_unknown_linux_gnu]\nrunner="custom"\n')
            with mock.patch.object(bench,"__file__",str(repo/"scripts/helper.py")),mock.patch.object(bench.subprocess,"check_output",return_value="host: test"):
                with self.assertRaisesRegex(SystemExit,"configuration unsupported"):bench._build_configuration()

    def test_atomic_replacement_failure_preserves_previous_manifest(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "scenarios.json"
            path.write_text("previous")
            with mock.patch.object(bench.os, "replace", side_effect=OSError("full")):
                with self.assertRaises(OSError): bench._write_atomic_private(path, "new")
            self.assertEqual(path.read_text(), "previous")
            self.assertEqual([p.name for p in Path(d).iterdir()], ["scenarios.json"])

    def test_capture_lock_prevents_pending_recovery_by_second_writer(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            args = type("Args", (), {"artifacts":str(root)})()
            def nested(_):
                with self.assertRaisesRegex(SystemExit, "(?:already active|unrecognized|empty)"): bench.snapshot(args)
                return 0
            with mock.patch.object(bench, "_snapshot_locked", side_effect=nested):
                self.assertEqual(bench.snapshot(args), 0)

    def test_signal_handlers_cleanup_and_restore(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "capture-manifest.json").write_text(json.dumps({"kind":"conversation-search-fixture"}))
            args = type("Args", (), {"artifacts":str(root),"label":"signal","timeout":10})()
            previous = bench.signal.getsignal(bench.signal.SIGTERM)
            def interrupted(_):
                bench.signal.getsignal(bench.signal.SIGTERM)(bench.signal.SIGTERM, None)
            with mock.patch.object(bench,"_validate_before_run"), mock.patch.object(bench, "_run_reserved", side_effect=interrupted):
                with self.assertRaises(KeyboardInterrupt): bench.run(args)
            self.assertIs(bench.signal.getsignal(bench.signal.SIGTERM), previous)
            self.assertFalse((root / "runs" / ".active-run.reserved").exists())

    def test_same_host_overlap_is_rejected(self):
        before = self._complete_run(started_at_unix=1, completed_at_unix=3)
        after = self._complete_run(started_at_unix=2, completed_at_unix=4)
        with tempfile.TemporaryDirectory() as d:
            root = Path(d); (root/"before.json").write_text(json.dumps(before)); (root/"after.json").write_text(json.dumps(after))
            with self.assertRaisesRegex(SystemExit, "overlap"):
                bench.compare(type("Args", (), {"before":str(root/"before.json"),"after":str(root/"after.json")})())

    def test_custom_cargo_home_config_is_hashed_without_contents(self):
        with tempfile.TemporaryDirectory() as d:
            config = Path(d) / "config.toml"; config.write_text('[build]\nrustflags=["-Copt-level=2"]\n')
            with mock.patch.dict("os.environ", {"CARGO_HOME":d}), mock.patch.object(bench.subprocess, "check_output", return_value="host: test"):
                recorded = bench._build_configuration()
            self.assertEqual(recorded["cargo_config_hashes"][str(config)], bench._hash(config))
            self.assertNotIn("opt-level", json.dumps(recorded))

    def test_snapshot_captures_checkpointed_offline_database_and_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.db"
            conn = sqlite3.connect(source)
            conn.execute("PRAGMA journal_mode=WAL")
            conn.execute("CREATE TABLE messages(message_id TEXT, content TEXT, display_data TEXT)")
            conn.execute("INSERT INTO messages VALUES ('m1', ?, NULL)",
                         ('{"tool_use_id":"call_w7yaFJY51rJxTjog4DKeE2wo","query":"exact observed"}',))
            conn.commit()
            out = root / "fixture"
            args = type("Args", (), {"source": str(source), "artifacts": str(out),
                                      "offline_snapshot": True, "force": False, "retries": 2, "busy_timeout": 1.0})()
            self.assertGreater(Path(f"{source}-wal").stat().st_size, 0)
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchall()
            self.assertEqual(conn.execute("PRAGMA journal_mode=DELETE").fetchone()[0], "delete")
            conn.close()
            offline = root / "offline.db"
            shutil.copyfile(source, offline)
            source = offline
            args.source = str(offline)
            self.assertFalse(Path(f"{source}-wal").exists())
            self.assertFalse(Path(f"{source}-shm").exists())
            bench.snapshot(args)
            self.assertEqual(sqlite3.connect(out / "captured.db").execute(
                "SELECT content FROM messages").fetchone()[0],
                '{"tool_use_id":"call_w7yaFJY51rJxTjog4DKeE2wo","query":"exact observed"}')
            manifest = json.loads((out / "capture-manifest.json").read_text())
            self.assertEqual(manifest["integrity_check"], "ok")
            self.assertEqual(len(manifest["schema_digest"]), 64)
            self.assertIsNone(manifest["migration_ledger"])
            self.assertEqual(manifest["recovered_queries"][0]["query"], "exact observed")
            source_conn = sqlite3.connect(source)
            self.assertEqual(
                manifest["logical_size_bytes"],
                source_conn.execute("PRAGMA page_count").fetchone()[0]
                * source_conn.execute("PRAGMA page_size").fetchone()[0],
            )
            source_conn.close()

    def test_recovery_uses_configured_replacement_after_partial_known_calls(self):
        replacement = "call_replacement_123"
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "9bc3b72d-extra", "known", json.dumps({"tool_use_id": bench.CALL_IDS[0], "query": "known query"}), 1))
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "replacement", "replacement-message", json.dumps({"tool_use_id": replacement, "query": "replacement query"}), 2))
        conn.commit()
        with mock.patch.dict("os.environ", {"PHOENIX_SEARCH_CALL_IDS": replacement, "PHOENIX_SEARCH_REPLACEMENT_TRANSCRIPT":"replacement"}):
            recovered = bench._recover_queries(conn)
        self.assertEqual([item["query"] for item in recovered], ["known query", "replacement query"])

    def test_recovery_prefix_predicate_is_indexable(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("CREATE INDEX messages_conversation ON messages(conversation_id)")
        plan = conn.execute(
            "EXPLAIN QUERY PLAN SELECT 1 FROM messages WHERE conversation_id GLOB ? LIMIT 1",
            ("9bc3*",),
        ).fetchall()
        self.assertIn("USING COVERING INDEX messages_conversation", plan[0][3])

    def test_recovery_does_not_full_scan_production_schema_without_configured_ids(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE conversations(id TEXT)")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "conv", "msg", json.dumps({"tool_use_id": bench.CALL_IDS[0], "query": "must not scan"}), 1))
        conn.commit()
        with mock.patch.dict("os.environ", {}, clear=True):
            self.assertEqual(bench._recover_queries(conn), [])

    def test_schema_evidence_captures_digest_and_existing_migration_ledger(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE _migrations(version INTEGER, name TEXT)")
        conn.execute("INSERT INTO _migrations VALUES (1, 'initial')")
        conn.commit()
        digest, ledger = bench._schema_evidence(conn)
        self.assertEqual(len(digest), 64)
        self.assertEqual(ledger, [{"version": 1, "name": "initial"}])

    def test_prepare_rejects_duplicate_normalized_observed_queries(self):
        self.assertEqual(bench._normalize_query("  Same   Query "), "same query")
        self.assertEqual(bench._normalize_query("same query"), "same query")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            db = root / "captured.db"
            db.write_bytes(b"fixture")
            fixture_hash = bench._hash(db)
            (root / "capture-manifest.json").write_text(json.dumps({
                "kind":"conversation-search-fixture", "snapshot_path": str(db),
                "sha256": fixture_hash, "size_bytes": db.stat().st_size,
                "source_path": str(root / "source.db"),
            }))
            args = type("Args", (), {"artifacts": str(root), "force": True})()
            with mock.patch.object(bench, "_recover_queries", return_value=[
                {"query": "Same   Query", "source_call_id": "one", "message_id": "m1"},
                {"query": "same query", "source_call_id": "two", "message_id": "m2"},
            ]), mock.patch.object(bench.sqlite3, "connect") as connect:
                connection = connect.return_value
                connection.execute.return_value.fetchone.return_value = None
                with self.assertRaisesRegex(SystemExit, "duplicate normalized"):
                    connect.return_value.execute.return_value.fetchone.side_effect = [(1,), (1000,), (0,)]
                    bench.prepare(args)

    def test_scenarios_are_bound_to_fixture_hash_at_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            db = root / "captured.db"
            db.write_bytes(b"fixture")
            fixture_hash = bench._hash(db)
            (root / "capture-manifest.json").write_text(json.dumps({
                "kind":"conversation-search-fixture", "snapshot_path": str(db), "sha256": fixture_hash, "size_bytes": db.stat().st_size,
                "schema_digest": "schema", "migration_ledger": [],
                "source_path": str(root / "source.db"),
            }))
            (root / "scenarios.json").write_text(json.dumps({
                "version": 1, "fixture_sha256": "different", "scenarios": [],
            }))
            with self.assertRaisesRegex(SystemExit, "does not match"):
                bench.run(type("Args", (), {"artifacts": str(root), "label": "suite", "force": False, "timeout": 1})())

    def test_force_run_removes_stale_result_and_publishes_temp_atomically(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            db = root / "captured.db"
            db.write_bytes(b"fixture")
            fixture_hash = bench._hash(db)
            (root / "capture-manifest.json").write_text(json.dumps({
                "kind":"conversation-search-fixture", "snapshot_path": str(db), "sha256": fixture_hash, "size_bytes": db.stat().st_size,
                "schema_digest": "schema", "migration_ledger": [],
                "source_path": str(root / "source.db"),
            }))
            (root / "scenarios.json").write_text(json.dumps({
                "version": 1, "fixture_sha256": fixture_hash, "expected_case_surface_set": [], "scenarios": [],
            }))
            runs = root / "runs"
            runs.mkdir()
            stale = runs / "suite.json"
            stale.write_text("stale")
            failure = runs / "failures" / "suite.json"
            failure.parent.mkdir()
            failure.write_text("old failure")

            class CompletedProcess:
                returncode = 0

                def wait(self, timeout=None):
                    return None

            def fake_popen(*args, **kwargs):
                blocked=bench.signal.pthread_sigmask(bench.signal.SIG_BLOCK, set())
                self.assertTrue({bench.signal.SIGTERM,bench.signal.SIGHUP,bench.signal.SIGINT}.issubset(blocked))
                self.assertIn("PHOENIX_SEARCH_BENCH_SCHEMA_DIGEST", kwargs["env"])
                self.assertIn("PHOENIX_SEARCH_BENCH_MIGRATION_LEDGER", kwargs["env"])
                Path(kwargs["env"]["PHOENIX_SEARCH_BENCH_OUT"]).write_text(json.dumps(self._complete_run()))
                return CompletedProcess()

            with mock.patch.object(bench, "_ensure_ignored_artifacts"), mock.patch.object(bench, "_ensure_clean_source"), mock.patch.object(
                bench, "_build_configuration", return_value={"rustc_version_verbose": "rustc", "cargo_version": "cargo", "target": "host", "profile": "release", "features": [], "environment": {}, "cargo_config_hashes":{}, "release_profile":{}, "native_compiler":{"path":"cc","sha256":"a"*64,"version":"version"}}
            ), mock.patch.object(bench, "_git_commit", return_value="commit"), mock.patch.object(bench.platform, "platform", return_value="platform"), mock.patch.object(
                bench.platform, "processor", return_value="processor"
            ), mock.patch.object(bench.subprocess, "Popen", side_effect=fake_popen):
                bench.run(type("Args", (), {"artifacts": str(root), "label": "suite", "force": True, "timeout": 1})())
            self.assertIn("samples",json.loads(stale.read_text()))
            self.assertTrue(failure.exists())
            self.assertEqual(failure.read_text(), "old failure")
            self.assertEqual(list(runs.glob(".*.tmp")), [])

    def test_run_refuses_dirty_source_tree(self):
        with mock.patch.object(bench.subprocess, "check_output", return_value=" M scripts/example.py\n"):
            with self.assertRaisesRegex(SystemExit, "dirty source"):
                bench._ensure_clean_source()

    def test_snapshot_cli_does_not_offer_custom_output(self):
        completed = subprocess.run(
            ["python", str(ROOT / "scripts/conversation_search_benchmark.py"), "snapshot", "--help"],
            capture_output=True, text=True, check=True,
        )
        self.assertNotIn("--output", completed.stdout)

    def test_recovery_matches_exact_tool_block_not_sibling(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "conv", "msg", json.dumps({"blocks": [
                {"tool_use_id": bench.CALL_IDS[0], "query": "the exact query"},
                {"tool_use_id": "other-call", "query": "must not leak"},
            ]}), 1))
        conn.commit()
        with mock.patch.dict("os.environ", {"PHOENIX_SEARCH_REPLACEMENT_TRANSCRIPT":"conv"}):
            self.assertEqual(bench._recover_queries(conn)[0]["query"], "the exact query")

    def test_recovery_decodes_serialized_arguments(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "conv", "msg", json.dumps({"tool_use_id": bench.CALL_IDS[0],
                                        "arguments": json.dumps({"query": "decoded query"})}), 1))
        conn.commit()
        with mock.patch.dict("os.environ", {"PHOENIX_SEARCH_REPLACEMENT_TRANSCRIPT":"conv"}):
            self.assertEqual(bench._recover_queries(conn)[0]["query"], "decoded query")

    def test_recovery_known_transcript_uses_bounded_conversation_lookup(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "9bc3b72d-extra", "msg", json.dumps({"tool_use_id": bench.CALL_IDS[0], "query": "indexed query"}), 1))
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            bench.KNOWN_CALLS[1]["conversation_id"], "msg2",
            json.dumps({"tool_use_id": bench.CALL_IDS[1], "query": "indexed query two"}), 2))
        conn.commit()
        self.assertEqual([item["query"] for item in bench._recover_queries(conn)],
                         ["indexed query", "indexed query two"])

    def test_snapshot_refuses_source_inside_artifacts(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            artifacts = root / "nested"
            artifacts.mkdir()
            source = artifacts / "source.db"
            sqlite3.connect(source).close()
            args = type("Args", (), {"source": str(source), "artifacts": str(artifacts),
                                      "offline_snapshot": True, "force": False, "retries": 1, "busy_timeout": 1.0})()
            with self.assertRaises(SystemExit):
                bench.snapshot(args)

    def test_snapshot_refuses_missing_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            args = type("Args", (), {"source": str(Path(tmp) / "missing"),
                                      "artifacts": tmp, "offline_snapshot": True, "force": False, "retries": 1,
                                      "busy_timeout": 1.0})()
            with self.assertRaises(SystemExit):
                bench.snapshot(args)

    def test_snapshot_uses_logical_capacity_guard(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.db"
            conn = sqlite3.connect(source)
            conn.execute("CREATE TABLE payload(value TEXT)")
            conn.execute("INSERT INTO payload VALUES (?)", ("x" * 4096,))
            conn.commit()
            conn.close()
            out = root / "fixture"
            args = type("Args", (), {"source": str(source), "artifacts": str(out),
                                      "offline_snapshot": True, "force": False, "retries": 1, "busy_timeout": 1.0,
                                      "deadline": 1.0})()
            logical_size = sqlite3.connect(source).execute("PRAGMA page_count").fetchone()[0] * sqlite3.connect(source).execute("PRAGMA page_size").fetchone()[0]
            with mock.patch.object(bench.shutil, "disk_usage", return_value=type("Usage", (), {"free": logical_size * 2 - 1})()):
                with self.assertRaisesRegex(SystemExit, "logical snapshot"):
                    bench.snapshot(args)

    def test_report_separates_runs_surfaces_and_discards_successful_warmup(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "capture-manifest.json").write_text(json.dumps({"kind":"conversation-search-fixture"}))
            runs = root / "runs"
            runs.mkdir()
            samples = self._complete_run()["samples"]
            samples.append({"case_id": "case", "surface": "retriever", "phase": "warmup_discarded",
                            "iteration": 0, "ok": True, "result_count":1, "result_identity":["id"], "result":"one", "result_digest":hashlib.sha256(b"one").hexdigest(), "duration_ms": 999})
            (runs / "suite-1.json").write_text(json.dumps(self._complete_run(samples=samples)))
            bench.report(type("Args", (), {"artifacts": str(root)})())
            report = (root / "report.md").read_text()
            self.assertNotIn("999.000", report)
            self.assertIn("suite-1.json — case — tool — warm", report)
            self.assertIn("fixture_sha256: a", report)
            self.assertIn("scenario_digest: s", report)

    def test_artifacts_inside_repo_must_be_git_ignored(self):
        with mock.patch.object(bench.subprocess, "check_output", return_value=""), mock.patch.object(bench.subprocess, "run", return_value=type("Result", (), {"returncode": 1})()):
            with self.assertRaisesRegex(SystemExit, "unignored"):
                bench._ensure_ignored_artifacts(ROOT / "not-ignored")

    def test_uri_percent_encodes_reserved_filename_characters(self):
        path = Path(tempfile.gettempdir()) / "db?name#fragment.sqlite"
        self.assertIn("%3Fname%23fragment.sqlite", bench._uri(path))

    def test_snapshot_validation_removes_temp_on_exception(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.db"
            sqlite3.connect(source).close()
            args = type("Args", (), {"source": str(source), "artifacts": str(root / "fixture"),
                                      "offline_snapshot": True, "force": False, "retries": 1, "busy_timeout": 1.0, "deadline": 1.0})()
            original = bench._counts
            try:
                bench._counts = mock.Mock(side_effect=RuntimeError("validation interrupted"))
                with self.assertRaisesRegex(RuntimeError, "validation interrupted"):
                    bench.snapshot(args)
                self.assertEqual(list((root / "fixture").glob(".captured.db.*.tmp")), [])
            finally:
                bench._counts = original

    def test_build_configuration_records_compiler_and_forwarded_environment(self):
        with mock.patch.object(bench.subprocess, "check_output", side_effect=["rustc\nhost: x86_64-test\n", "cargo 1", "native compiler"]):
            with mock.patch.dict("os.environ", {"RUSTFLAGS": "-C opt-level=3", "CARGO_BUILD_TARGET": "wasm32"}, clear=True):
                with self.assertRaisesRegex(SystemExit,"cross native compiler"):
                    bench._build_configuration()

    def test_selective_term_requires_bounded_nonzero_match(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE VIRTUAL TABLE message_fts USING fts5(body)")
        conn.executemany("INSERT INTO message_fts(body) VALUES (?)", [("rareterm",), ("rareterm",)])
        self.assertEqual(bench._selective_term(conn, "rareterm common"), "rareterm")

    def test_prepare_uses_single_alphanumeric_no_hit_token(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            db = root / "captured.db"
            db.write_bytes(b"fixture")
            fixture_hash = bench._hash(db)
            (root / "capture-manifest.json").write_text(json.dumps({
                "kind":"conversation-search-fixture", "snapshot_path": str(db),
                "sha256": fixture_hash, "size_bytes": db.stat().st_size,
                "source_path": str(root / "source.db"),
            }))
            args = type("Args", (), {"artifacts": str(root), "force": True})()
            with mock.patch.object(bench, "_recover_queries", return_value=[
                {"query": "first query", "source_call_id": "one", "message_id": "m1"},
                {"query": "second query", "source_call_id": "two", "message_id": "m2"},
            ]), mock.patch.object(bench.sqlite3, "connect") as connect:
                connect.return_value.execute.return_value.fetchone.return_value = ("conv",)
                connect.return_value.execute.return_value.fetchall.return_value = [(1,)]
                with mock.patch.object(bench, "_selective_term", return_value="verifiedterm"):
                    connect.return_value.execute.return_value.fetchone.side_effect = [(1,), (1000,), (0,)]
                    bench.prepare(args)
            scenarios = json.loads((root / "scenarios.json").read_text())["scenarios"]
            no_hit = next(item for item in scenarios if item["id"] == "verified-no-hit")
            self.assertRegex(no_hit["query"], r"^[A-Za-z0-9]+$")

    def test_compare_refuses_measurement_regime_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            base = self._complete_run()
            before.write_text(json.dumps(base))
            after.write_text(json.dumps({**base, "run_uuid":"other", "started_at_unix":0.0,"completed_at_unix":1.0, "runtime":{"worker_threads":3,"measurement_clock":"monotonic"}}))
            with self.assertRaisesRegex(SystemExit, "regime"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())

    def test_compare_refuses_missing_metadata_and_output_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            metadata = self._complete_run()
            before.write_text(json.dumps(metadata))
            incomplete = dict(metadata)
            incomplete.pop("commit")
            after.write_text(json.dumps(incomplete))
            with self.assertRaisesRegex(SystemExit, "missing metadata"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())
            changed = [dict(sample) for sample in metadata["samples"]]
            changed[0]["result_digest"] = "two"
            after.write_text(json.dumps({**metadata, "samples": changed}))
            with self.assertRaisesRegex(SystemExit, "(?:output mismatch|digest mismatch)"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())

    def test_compare_refuses_incomplete_warm_counts(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            run = self._complete_run(samples=self._complete_run()["samples"][:-1])
            before.write_text(json.dumps(run))
            after.write_text(json.dumps(self._complete_run()))
            with self.assertRaisesRegex(SystemExit, "incomplete warm"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())

    def test_run_rejects_path_traversal_label(self):
        with self.assertRaisesRegex(SystemExit, "label"):
            bench._label("../escape")

    def test_compare_refuses_fixture_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            before.write_text(json.dumps(self._complete_run()))
            after.write_text(json.dumps(self._complete_run(fixture_sha256="b")))
            with self.assertRaisesRegex(SystemExit, "regime"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())


if __name__ == "__main__":
    unittest.main()
