import contextlib
import importlib.util
import multiprocessing
import os
import platform
import sys
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_devpy():
    spec = importlib.util.spec_from_file_location("devpy_cache_under_test", ROOT / "dev.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class CompilerCacheTests(unittest.TestCase):
    def setUp(self):
        self.dev = load_devpy()

    def configure(self, requested=None, *, env=None, installed=(), **options):
        env = {} if env is None else env
        with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(
            self.dev.shutil,
            "which",
            side_effect=lambda name: f"/bin/{name}" if name in installed else None,
        ), mock.patch.object(
            self.dev,
            "_kache_version",
            return_value=("0.26.0", None),
        ), mock.patch.object(
            self.dev,
            "_usable_sccache",
            return_value=("sccache 0.18.0", None)
            if "sccache" in installed
            else (None, "not installed or not on PATH"),
        ), mock.patch.object(
            self.dev, "_kache_host_error", return_value=None
        ), mock.patch.object(self.dev, "_ensure_kache_daemon", return_value=None):
            selected = self.dev._configure_compiler_cache(requested, **options)
            return selected, os.environ.copy()

    def test_all_cargo_lanes_enable_compiler_cache_setup(self):
        for lane in ("rust", "clippy", "e2e"):
            with self.subTest(lane=lane):
                self.assertTrue(self.dev._cargo_check_active({lane}))

    def test_non_cargo_lane_skips_compiler_cache_setup(self):
        self.assertFalse(self.dev._cargo_check_active({"vitest"}))

    def test_explicit_rustc_wrapper_wins(self):
        with mock.patch("builtins.print") as output:
            selected, env = self.configure("kache", env={"RUSTC_WRAPPER": "custom"})
        output.assert_called_once_with("  Compiler cache: explicit")
        self.assertEqual("explicit", selected)
        self.assertEqual("custom", env["RUSTC_WRAPPER"])

    def test_auto_preserves_sccache_until_kache_debug_fidelity_is_qualified(self):
        with mock.patch("builtins.print") as output:
            selected, env = self.configure(installed={"kache", "sccache"})
        self.assertEqual("sccache", selected)
        self.assertEqual(
            str(self.dev.Path("/bin/sccache").resolve()), env["RUSTC_WRAPPER"]
        )
        output.assert_any_call(
            "  ⚠ kache unavailable; using sccache: "
            "requires explicit opt-in because restored-archive debug-symbol fidelity is unqualified"
        )

    def test_explicit_kache_remains_opt_in_with_fidelity_warning(self):
        with mock.patch("builtins.print") as output:
            selected, env = self.configure(
                "kache",
                env={"KACHE_LOG_FILE": "kache=trace"},
                installed={"kache", "sccache"},
            )
        self.assertEqual("kache", selected)
        output.assert_any_call(
            "  ⚠ kache restored-archive source-level debug fidelity is unqualified"
        )
        self.assertEqual(
            str(self.dev.Path("/bin/kache").resolve()), env["RUSTC_WRAPPER"]
        )
        self.assertEqual("kache=trace", env["KACHE_LOG_FILE"])

    def test_kache_host_support_is_limited_to_darwin_arm64(self):
        with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(
            self.dev.platform, "machine", return_value="arm64"
        ):
            self.assertIsNone(self.dev._kache_host_error())

        for system, machine in (("linux", "aarch64"), ("win32", "AMD64"), ("darwin", "x86_64")):
            with self.subTest(system=system, machine=machine), mock.patch.object(
                self.dev.sys, "platform", system
            ), mock.patch.object(self.dev.platform, "machine", return_value=machine):
                error = self.dev._kache_host_error()
                self.assertIn(f"unsupported host {system}/{machine.lower()}", error or "")
                self.assertIn("qualified host is darwin/arm64", error or "")

    def test_explicit_kache_rejects_unsupported_host_before_version_probe(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev, "_kache_host_error", return_value="unsupported host linux/x86_64"
        ), mock.patch.object(self.dev, "_kache_binary", return_value="/bin/kache"), mock.patch.object(
            self.dev, "_kache_version"
        ) as version:
            with self.assertRaisesRegex(SystemExit, "unsupported host linux/x86_64"):
                self.dev._configure_compiler_cache("kache")
        version.assert_not_called()

    def test_auto_reports_none_when_no_backend_is_installed(self):
        with mock.patch("builtins.print") as output:
            selected, env = self.configure(installed=set())
        self.assertEqual("none", selected)
        self.assertNotIn("RUSTC_WRAPPER", env)
        output.assert_any_call("  Compiler cache: none")

    def test_sccache_limit_warning_reports_running_server_mismatch(self):
        completed = mock.Mock(
            returncode=0,
            stdout='{"max_cache_size": 21474836480}',
            stderr="",
        )
        with mock.patch.object(self.dev.subprocess, "run", return_value=completed):
            warning = self.dev._sccache_limit_warning()

        self.assertIn("20.0 GiB", warning)
        self.assertIn("restart sccache", warning)

    def test_sccache_limit_warning_accepts_effective_limit(self):
        completed = mock.Mock(
            returncode=0,
            stdout='{"max_cache_size": 10737418240}',
            stderr="",
        )
        with mock.patch.object(self.dev.subprocess, "run", return_value=completed):
            self.assertIsNone(self.dev._sccache_limit_warning())

    def test_sccache_limit_warning_honors_explicit_limit(self):
        completed = mock.Mock(
            returncode=0,
            stdout='{"max_cache_size": 21474836480}',
            stderr="",
        )
        with mock.patch.object(self.dev.subprocess, "run", return_value=completed):
            self.assertIsNone(self.dev._sccache_limit_warning("20G"))

    def test_sccache_limit_warning_reports_invalid_config(self):
        warning = self.dev._sccache_limit_warning("twenty gigs")
        self.assertIn("unsupported cache size", warning)

    def test_auto_does_not_use_unqualified_kache_when_sccache_is_unavailable(self):
        selected, env = self.configure(installed={"kache"})
        self.assertEqual("none", selected)
        self.assertNotIn("RUSTC_WRAPPER", env)

    def test_none_disables_automatic_wrapper(self):
        selected, env = self.configure("none", installed={"kache", "sccache"})
        self.assertEqual("none", selected)
        self.assertNotIn("RUSTC_WRAPPER", env)

    def test_environment_selects_backend(self):
        selected, env = self.configure(
            env={"PHOENIX_COMPILER_CACHE": "kache"}, installed={"kache"}
        )
        self.assertEqual("kache", selected)
        self.assertEqual(str(self.dev.Path("/bin/kache").resolve()), env["RUSTC_WRAPPER"])

    def test_cli_selection_takes_precedence_over_environment(self):
        selected, env = self.configure(
            "sccache",
            env={"PHOENIX_COMPILER_CACHE": "kache"},
            installed={"kache", "sccache"},
        )
        self.assertEqual("sccache", selected)
        self.assertEqual(str(self.dev.Path("/bin/sccache").resolve()), env["RUSTC_WRAPPER"])

    def test_explicit_sccache_does_not_probe_kache(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil,
            "which",
            side_effect=lambda name: f"/bin/{name}",
        ), mock.patch.object(
            self.dev, "_usable_sccache", return_value=("sccache 0.18.0", None)
        ), mock.patch.object(self.dev, "_kache_version") as probe:
            self.assertEqual("sccache", self.dev._configure_compiler_cache("sccache"))
        probe.assert_not_called()

    @unittest.skipUnless(
        sys.platform == "darwin" and platform.machine().lower() == "arm64",
        "Kache v0.26.0 is qualified only on macOS arm64",
    )
    def test_local_kache_binary_is_supported(self):
        with mock.patch.dict(
            os.environ, {"PHOENIX_KACHE_BIN": "/opt/local/kache"}, clear=True
        ), mock.patch.object(self.dev.Path, "is_file", return_value=True), mock.patch.object(
            self.dev.os, "access", return_value=True
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=("0.26.0", None)
        ), mock.patch.object(
            self.dev, "_ensure_kache_daemon", return_value=None
        ) as ensure:
            selected = self.dev._configure_compiler_cache("kache")
            self.assertEqual("kache", selected)
            self.assertEqual("/opt/local/kache", os.environ["RUSTC_WRAPPER"])
            ensure.assert_called_once_with("/opt/local/kache", cargo_cwd=None)

    def test_minimal_kache_configuration_gets_generated_socket(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev, "_private_kache_socket_dir", return_value=Path("/tmp/private-kache")
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", return_value=contextlib.nullcontext()
        ), mock.patch.object(
            self.dev, "_start_kache_daemon_locked", return_value=None
        ):
            self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))
            self.assertRegex(
                os.environ["KACHE_SOCKET_PATH"],
                r"^/tmp/private-kache/[0-9a-f]{16}\.sock$",
            )

    def test_config_override_gets_generated_socket(self):
        with mock.patch.dict(
            os.environ, {"KACHE_CONFIG": "/tmp/config.toml"}, clear=True
        ), mock.patch.object(
            self.dev, "_private_kache_socket_dir", return_value=Path("/tmp/private-kache")
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", return_value=contextlib.nullcontext()
        ), mock.patch.object(
            self.dev, "_start_kache_daemon_locked", return_value=None
        ):
            self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))
            self.assertIn("KACHE_SOCKET_PATH", os.environ)
            self.assertEqual("/tmp/config.toml", os.environ["KACHE_CONFIG"])

    def test_kache_daemon_uses_cargo_working_directory(self):
        completed = mock.Mock(returncode=0, stdout="", stderr="")
        cargo_cwd = self.dev.Path("/detached/build")
        daemon_env = {}

        def run_daemon(*_args, **kwargs):
            daemon_env.update(kwargs["env"])
            return completed

        with mock.patch.dict(
            os.environ,
            {
                "KACHE_LOG_FILE": "kache=trace",
                "KACHE_LOG_FILE_PATH": "/tmp/kache.log",
                "KACHE_SOCKET_PATH": "/tmp/kache.sock",
            },
            clear=True,
        ), mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "run", side_effect=run_daemon
        ) as run, mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value=None
        ) as wait:
            self.assertIsNone(
                self.dev._ensure_kache_daemon("/bin/kache", cargo_cwd=cargo_cwd)
            )
        self.assertEqual(cargo_cwd, run.call_args.kwargs["cwd"])
        self.assertEqual("kache=trace", daemon_env["KACHE_LOG_FILE"])
        self.assertEqual("/tmp/kache.log", daemon_env["KACHE_LOG_FILE_PATH"])
        wait.assert_called_once_with("/bin/kache", cargo_cwd=cargo_cwd)

    def test_kache_lock_setup_failure_is_actionable(self):
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/unwritable/kache.sock"}, clear=True
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", side_effect=OSError("permission denied")
        ):
            error = self.dev._ensure_kache_daemon("/bin/kache")
        self.assertEqual("cannot lock Kache socket setup: permission denied", error)

    def test_kache_lock_creation_failure_propagates(self):
        with mock.patch.object(
            self.dev.Path, "mkdir", side_effect=OSError("read-only filesystem")
        ):
            with self.assertRaisesRegex(OSError, "read-only filesystem"):
                with self.dev._kache_socket_lock(Path("/tmp/kache.sock")):
                    self.fail("lock body must not run")

    def test_kache_lock_acquisition_failure_closes_descriptor(self):
        with mock.patch.object(self.dev.os, "open", return_value=42), mock.patch.object(
            self.dev.fcntl, "flock", side_effect=OSError("lock unavailable")
        ), mock.patch.object(self.dev.os, "close") as close:
            with self.assertRaisesRegex(OSError, "lock unavailable"):
                with self.dev._kache_socket_lock(Path("/tmp/kache.sock")):
                    self.fail("lock body must not run")
        close.assert_called_once_with(42)

    def test_kache_daemon_start_replaces_undecodable_output(self):
        completed = mock.Mock(returncode=1, stdout="\ufffd", stderr="")
        with mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "run", return_value=completed
        ) as run:
            error = self.dev._start_kache_daemon_locked("/bin/kache", cargo_cwd=None)
        self.assertEqual("\ufffd", error)
        self.assertEqual("replace", run.call_args.kwargs["errors"])

    def test_kache_daemon_holds_socket_lock_across_check_start_and_readiness(self):
        completed = mock.Mock(returncode=0, stdout="", stderr="")
        events = []

        @contextlib.contextmanager
        def lock(_socket):
            events.append("lock")
            yield
            events.append("unlock")

        def status(*_args, **_kwargs):
            events.append("check")
            return False, None

        def start(*_args, **_kwargs):
            events.append("start")
            return completed

        def ready(*_args, **_kwargs):
            events.append("ready")
            return None

        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/tmp/kache.sock"}, clear=True
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", side_effect=lock
        ), mock.patch.object(
            self.dev, "_kache_daemon_is_running", side_effect=status
        ), mock.patch.object(
            self.dev.subprocess, "run", side_effect=start
        ), mock.patch.object(
            self.dev, "_wait_for_kache_daemon", side_effect=ready
        ):
            self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))
        self.assertEqual(["lock", "check", "start", "ready", "unlock"], events)

    @unittest.skipUnless("fork" in multiprocessing.get_all_start_methods(), "requires fork")
    def test_kache_daemon_serializes_cross_process_contenders(self):
        context = multiprocessing.get_context("fork")
        start = context.Event()
        ready = context.Queue()
        results = context.Queue()

        with self.dev.tempfile.TemporaryDirectory() as temporary:
            socket = Path(temporary) / "kache.sock"
            marker = Path(temporary) / "daemon-environment"

            def contend(candidate):
                dev = load_devpy()

                def status(*_args, **_kwargs):
                    return marker.exists(), None

                def launch(*_args, **_kwargs):
                    marker.write_text(candidate)
                    return mock.Mock(returncode=0, stdout="", stderr="")

                with mock.patch.dict(
                    os.environ,
                    {
                        "KACHE_SOCKET_PATH": str(socket),
                        "KACHE_REMOTE_BUCKET": candidate,
                    },
                    clear=True,
                ), mock.patch.object(
                    dev, "_kache_daemon_is_running", side_effect=status
                ), mock.patch.object(
                    dev.subprocess, "run", side_effect=launch
                ), mock.patch.object(dev, "_wait_for_kache_daemon", return_value=None):
                    ready.put(candidate)
                    start.wait()
                    results.put((candidate, dev._ensure_kache_daemon("/bin/kache")))

            contenders = [
                context.Process(target=contend, args=(candidate,))
                for candidate in ("bucket-a", "bucket-b")
            ]
            for contender in contenders:
                contender.start()
            self.assertEqual({ready.get(timeout=5), ready.get(timeout=5)}, {"bucket-a", "bucket-b"})
            start.set()
            outcomes = dict(results.get(timeout=5) for _ in contenders)
            for contender in contenders:
                contender.join(timeout=5)
                self.assertEqual(0, contender.exitcode)

            winner = marker.read_text()
            loser = "bucket-b" if winner == "bucket-a" else "bucket-a"
            self.assertIsNone(outcomes[winner])
            self.assertIn("environment cannot be verified", outcomes[loser] or "")
            self.assertTrue(socket.with_name(f"{socket.name}.lock").exists())

    def test_kache_daemon_rejects_running_process_with_unverifiable_environment(self):
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/tmp/kache.sock"}, clear=True
        ), mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(True, None)
        ), mock.patch.object(self.dev.subprocess, "run") as run:
            error = self.dev._ensure_kache_daemon("/bin/kache")
        self.assertIn("environment cannot be verified", error or "")
        self.assertIn("kache daemon stop", error or "")
        run.assert_not_called()

    def test_kache_daemon_rejects_unknown_existing_state(self):
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/tmp/kache.sock"}, clear=True
        ), mock.patch.object(
            self.dev,
            "_kache_daemon_is_running",
            return_value=(False, "daemon readiness response had an invalid shape"),
        ), mock.patch.object(self.dev.subprocess, "run") as run:
            error = self.dev._ensure_kache_daemon("/bin/kache")
        self.assertIn("cannot verify existing daemon environment", error or "")
        self.assertIn("kache daemon stop", error or "")
        run.assert_not_called()

    def test_kache_readiness_polls_until_running(self):
        starting = mock.Mock(
            returncode=0,
            stdout='{"daemon_running":false,"socket":null}',
            stderr="",
        )
        running = mock.Mock(
            returncode=0,
            stdout='{"daemon_running":true,"socket":"/tmp/kache.sock"}',
            stderr="",
        )
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.subprocess, "run", side_effect=(starting, running)
        ), mock.patch.object(self.dev.time, "sleep"):
            self.assertIsNone(self.dev._wait_for_kache_daemon("/bin/kache", timeout=1))

    def test_kache_readiness_replaces_undecodable_output(self):
        status = mock.Mock(returncode=0, stdout="\ufffd", stderr="")
        clock = iter((0.0, 0.0, 1.0))
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.subprocess, "run", return_value=status
        ) as run, mock.patch.object(
            self.dev.time, "monotonic", side_effect=lambda: next(clock)
        ), mock.patch.object(self.dev.time, "sleep"):
            error = self.dev._wait_for_kache_daemon("/bin/kache", timeout=0.5)
        self.assertIn("Expecting value", error or "")
        self.assertEqual("replace", run.call_args.kwargs["errors"])

    def test_kache_readiness_rejects_non_object_status(self):
        status = mock.Mock(returncode=0, stdout="null", stderr="")
        clock = iter((0.0, 0.0, 1.0))
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.subprocess, "run", return_value=status
        ), mock.patch.object(self.dev.time, "monotonic", side_effect=lambda: next(clock)), mock.patch.object(
            self.dev.time, "sleep"
        ):
            error = self.dev._wait_for_kache_daemon("/bin/kache", timeout=0.5)
        self.assertEqual("daemon readiness response was not an object", error)

    def test_kache_readiness_rejects_non_string_socket(self):
        status = mock.Mock(
            returncode=0,
            stdout='{"daemon_running":true,"socket":[]}',
            stderr="",
        )
        clock = iter((0.0, 0.0, 1.0))
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.subprocess, "run", return_value=status
        ), mock.patch.object(self.dev.time, "monotonic", side_effect=lambda: next(clock)), mock.patch.object(
            self.dev.time, "sleep"
        ):
            error = self.dev._wait_for_kache_daemon("/bin/kache", timeout=0.5)
        self.assertEqual("daemon readiness socket was not a string", error)

    def test_kache_readiness_rejects_wrong_socket(self):
        running = mock.Mock(
            returncode=0,
            stdout='{"daemon_running":true,"socket":"/tmp/other.sock"}',
            stderr="",
        )
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/tmp/expected.sock"}, clear=True
        ), mock.patch.object(self.dev.subprocess, "run", return_value=running):
            error = self.dev._wait_for_kache_daemon("/bin/kache", timeout=1)
        self.assertIn("unexpected socket", error or "")

    def test_daemon_socket_uses_private_owned_directory(self):
        completed = mock.Mock(returncode=0, stdout="", stderr="")
        with self.subTest("socket path and permissions"), mock.patch.dict(
            os.environ, {"KACHE_CACHE_DIR": "/very/long/worktree/cache"}, clear=True
        ), mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "run", return_value=completed
        ) as run, mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value=None
        ):
            with self.dev.tempfile.TemporaryDirectory() as temporary:
                with mock.patch.object(self.dev.tempfile, "gettempdir", return_value=temporary):
                    self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))
                socket_path = Path(os.environ["KACHE_SOCKET_PATH"])
                self.assertEqual(socket_path.parent.name, f"phoenix-kache-{os.getuid()}")
                self.assertEqual(socket_path.parent.stat().st_mode & 0o777, 0o700)
                self.assertRegex(socket_path.name, r"^[0-9a-f]{16}\.sock$")
            run.assert_called_once()

    def test_auto_reports_explicit_opt_in_requirement_before_sccache_fallback(self):
        with mock.patch("builtins.print") as output:
            selected, _ = self.configure(installed={"sccache"})
        self.assertEqual("sccache", selected)
        output.assert_any_call(
            "  ⚠ kache unavailable; using sccache: "
            "requires explicit opt-in because restored-archive debug-symbol fidelity is unqualified"
        )

    def test_explicit_kache_reports_invalid_configured_binary(self):
        with self.assertRaisesRegex(SystemExit, "PHOENIX_KACHE_BIN.*missing/kache"):
            self.configure(
                "kache",
                env={"PHOENIX_KACHE_BIN": "missing/kache"},
                installed=set(),
            )

    def test_kache_disabled_auto_falls_back_to_sccache(self):
        selected, env = self.configure(
            env={"KACHE_DISABLED": "1"}, installed={"kache", "sccache"}
        )
        self.assertEqual("sccache", selected)
        self.assertEqual(str(self.dev.Path("/bin/sccache").resolve()), env["RUSTC_WRAPPER"])

    def test_kache_disabled_explicit_fails(self):
        with self.assertRaisesRegex(SystemExit, "KACHE_DISABLED"):
            self.configure("kache", env={"KACHE_DISABLED": "1"}, installed={"kache"})

    def test_sccache_probe_requires_recognizable_version(self):
        for output in ("exit code 0", "", "other 0.18.0"):
            with self.subTest(output=output), mock.patch.object(
                self.dev,
                "_command_version",
                return_value=(output, None),
            ):
                version, error = self.dev._usable_sccache("/bin/sccache")
                self.assertIsNone(version)
                self.assertIn("unrecognized version output", error or "")

    def test_explicit_sccache_rejects_failed_version_probe(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", return_value="/bin/sccache"
        ), mock.patch.object(
            self.dev, "_usable_sccache", return_value=(None, "bad architecture")
        ):
            with self.assertRaisesRegex(SystemExit, "bad architecture"):
                self.dev._configure_compiler_cache("sccache")

    def test_auto_never_probes_or_starts_kache(self):
        with mock.patch("builtins.print"), mock.patch.dict(
            os.environ, {}, clear=True
        ), mock.patch.object(
            self.dev.shutil, "which", return_value=None
        ), mock.patch.object(self.dev, "_kache_version") as version, mock.patch.object(
            self.dev, "_ensure_kache_daemon"
        ) as daemon:
            self.assertEqual("none", self.dev._configure_compiler_cache("auto"))
        version.assert_not_called()
        daemon.assert_not_called()

    def test_explicit_kache_fails_when_daemon_fails(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: "/bin/kache" if name == "kache" else None
        ), mock.patch.object(
            self.dev, "_kache_host_error", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=("0.26.0", None)
        ), mock.patch.object(self.dev, "_ensure_kache_daemon", return_value="socket failed"):
            with self.assertRaisesRegex(SystemExit, "kache daemon failed to start: socket failed"):
                self.dev._configure_compiler_cache("kache")

    def test_unavailable_explicit_backend_fails(self):
        with self.assertRaisesRegex(SystemExit, "kache.*not installed"):
            self.configure("kache")

    def test_command_version_replaces_undecodable_output(self):
        completed = mock.Mock(returncode=0, stdout="kache \ufffd", stderr="")
        with mock.patch.object(self.dev.subprocess, "run", return_value=completed) as run:
            detail, error = self.dev._command_version("/bin/kache")
        self.assertEqual("kache \ufffd", detail)
        self.assertIsNone(error)
        self.assertEqual("replace", run.call_args.kwargs["errors"])

    def test_kache_version_accepts_qualified_release(self):
        with mock.patch.object(
            self.dev, "_command_version", return_value=("kache 0.26.0", None)
        ):
            self.assertEqual(("0.26.0", None), self.dev._kache_version("/bin/kache"))

    def test_kache_version_rejects_unqualified_patch_and_prerelease(self):
        for output, expected in (
            ("kache 0.26.1", "unsupported"),
            ("kache 0.26.0-rc1", "unrecognized"),
        ):
            with self.subTest(output=output), mock.patch.object(
                self.dev, "_command_version", return_value=(output, None)
            ):
                version, error = self.dev._kache_version("/bin/kache")
                self.assertIsNone(version)
                self.assertIn(expected, error or "")

    def test_explicit_kache_rejects_unsupported_release(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: "/bin/kache" if name == "kache" else None
        ), mock.patch.object(
            self.dev, "_kache_host_error", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=(None, "unsupported kache 0.25.0")
        ):
            with self.assertRaisesRegex(SystemExit, "incompatible.*unsupported kache 0.25.0"):
                self.dev._configure_compiler_cache("kache")

    def test_auto_skips_unsupported_kache_for_sccache(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: f"/bin/{name}"
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=(None, "unsupported kache 0.25.0")
        ), mock.patch.object(
            self.dev, "_usable_sccache", return_value=("sccache 0.18.0", None)
        ):
            self.assertEqual("sccache", self.dev._configure_compiler_cache("auto"))
            self.assertEqual(
                str(self.dev.Path("/bin/sccache").resolve()),
                os.environ["RUSTC_WRAPPER"],
            )

    def test_build_configures_cache_before_spawning_cargo(self):
        calls = []
        stderr = mock.Mock()
        stderr.__iter__ = mock.Mock(return_value=iter(()))
        process = mock.Mock(stderr=stderr, returncode=0)
        process.wait.return_value = 0
        process.poll.return_value = 0
        with mock.patch.object(
            self.dev,
            "_configure_compiler_cache",
            side_effect=lambda _requested, **_kwargs: calls.append("cache"),
        ), mock.patch.object(
            self.dev.subprocess,
            "Popen",
            side_effect=lambda *args, **kwargs: calls.append(("cargo", kwargs.get("env"))) or process,
        ), mock.patch.object(self.dev, "_begin_dev_span", return_value=None), mock.patch.object(
            self.dev, "_finish_dev_span"
        ):
            self.dev.build_rust()
        self.assertEqual("cache", calls[0])
        self.assertEqual("cargo", calls[1][0])
        self.assertIsNotNone(calls[1][1])

    def test_local_prod_deploy_reuses_one_compiler_cache_setup(self):
        setup = (
            "kache",
            {
                "RUSTC_WRAPPER": "/bin/kache",
                "KACHE_SOCKET_PATH": "/tmp/kache.sock",
            },
        )
        controller = self.dev.ProdDeployControllerOptions(backend="launchd")
        with mock.patch.object(
            self.dev, "detect_prod_env", return_value="launchd"
        ), mock.patch.object(
            self.dev, "_compiler_cache_subprocess_env", return_value=setup
        ) as configure, mock.patch.object(self.dev, "cmd_check") as check, mock.patch.object(
            self.dev, "launchd_prod_deploy"
        ) as deploy, mock.patch.object(self.dev, "native_prod_deploy") as native, mock.patch.object(
            self.dev, "prod_daemon_deploy"
        ) as daemon:
            self.dev.cmd_prod_deploy(controller=controller)

        configure.assert_called_once_with(cargo_cwd=self.dev.ROOT)
        self.assertIs(setup, check.call_args.kwargs["compiler_cache_setup"])
        self.assertIs(setup, deploy.call_args.kwargs["compiler_cache_setup"])
        native.assert_not_called()
        daemon.assert_not_called()

    def test_independent_prod_deploys_configure_independently(self):
        controller = self.dev.ProdDeployControllerOptions(backend="launchd")
        with mock.patch.object(
            self.dev, "detect_prod_env", return_value="launchd"
        ), mock.patch.object(
            self.dev,
            "_compiler_cache_subprocess_env",
            side_effect=[("kache", {"run": "one"}), SystemExit("existing daemon")],
        ) as configure, mock.patch.object(self.dev, "cmd_check"), mock.patch.object(
            self.dev, "launchd_prod_deploy"
        ), mock.patch.object(self.dev, "native_prod_deploy") as native, mock.patch.object(
            self.dev, "prod_daemon_deploy"
        ) as daemon:
            self.dev.cmd_prod_deploy(controller=controller)
            with self.assertRaisesRegex(SystemExit, "existing daemon"):
                self.dev.cmd_prod_deploy(controller=controller)
        self.assertEqual(2, configure.call_count)
        native.assert_not_called()
        daemon.assert_not_called()

    def test_subprocess_environment_reports_actual_backend_without_leaking(self):
        def configure(_requested, **_options):
            os.environ["RUSTC_WRAPPER"] = "/bin/sccache"
            return "sccache"

        with mock.patch.dict(os.environ, {"ORIGINAL": "yes"}, clear=True), mock.patch.object(
            self.dev, "_configure_compiler_cache", side_effect=configure
        ):
            selected, environment = self.dev._compiler_cache_subprocess_env("auto")
            self.assertEqual({"ORIGINAL": "yes"}, os.environ)
        self.assertEqual("sccache", selected)
        self.assertEqual("/bin/sccache", environment["RUSTC_WRAPPER"])

    def test_check_cache_overrides_are_backend_specific(self):
        configured = {
            "RUSTC_WRAPPER": "/bin/kache",
            "KACHE_CACHE_DIR": "/kache",
            "SCCACHE_DIR": "/sccache",
            "UNRELATED": "value",
        }
        self.assertEqual(
            {
                "RUSTC_WRAPPER": "/bin/kache",
                "KACHE_CACHE_DIR": "/kache",
            },
            self.dev._compiler_cache_overrides("kache", configured),
        )
        self.assertEqual(
            {
                "RUSTC_WRAPPER": "/bin/kache",
                "SCCACHE_DIR": "/sccache",
            },
            self.dev._compiler_cache_overrides("sccache", configured),
        )

    def test_only_cargo_owning_steps_receive_check_cache(self):
        self.assertTrue(self.dev._command_uses_compiler_cache(["cargo", "test"]))
        self.assertTrue(self.dev._command_uses_compiler_cache(["/opt/bin/cargo", "clippy"]))
        self.assertTrue(
            self.dev._command_uses_compiler_cache(["uv", "run", "tests/e2e/run.py"])
        )
        self.assertFalse(self.dev._command_uses_compiler_cache(["uv", "run", "tests.py"]))
        self.assertFalse(self.dev._command_uses_compiler_cache([]))

    def test_relative_wrapper_path_is_made_absolute(self):
        with mock.patch.object(self.dev.Path, "resolve", return_value=self.dev.Path("/workspace/bin/kache")):
            self.assertEqual(
                "/workspace/bin/kache", self.dev._absolute_executable("bin/kache")
            )

    def test_cache_paths_normalize_against_invoking_directory(self):
        with mock.patch.dict(
            os.environ,
            {
                "KACHE_CACHE_DIR": "cache/kache",
                "KACHE_SOCKET_PATH": "run/kache.sock",
                "KACHE_CONFIG": "config/kache.toml",
                "KACHE_HOST_CONFIG": "config/host.toml",
                "KACHE_RUNTIME_DIR": "run/kache",
                "KACHE_LOG_FILE": "kache=trace",
                "KACHE_LOG_FILE_PATH": "logs/kache.log",
                "SCCACHE_DIR": "cache/sccache",
                "SCCACHE_CONF": "config/sccache.toml",
                "SCCACHE_ERROR_LOG": "logs/sccache.log",
                "SCCACHE_GCS_KEY_PATH": "credentials/gcs.json",
            },
            clear=True,
        ):
            self.dev._normalize_cache_paths("kache", self.dev.Path("/workspace"))
            self.assertEqual("/workspace/cache/kache", os.environ["KACHE_CACHE_DIR"])
            self.assertEqual("/workspace/run/kache.sock", os.environ["KACHE_SOCKET_PATH"])
            self.assertEqual("/workspace/config/kache.toml", os.environ["KACHE_CONFIG"])
            self.assertEqual("/workspace/config/host.toml", os.environ["KACHE_HOST_CONFIG"])
            self.assertEqual("/workspace/run/kache", os.environ["KACHE_RUNTIME_DIR"])
            self.assertEqual("kache=trace", os.environ["KACHE_LOG_FILE"])
            self.assertEqual("/workspace/logs/kache.log", os.environ["KACHE_LOG_FILE_PATH"])
            self.assertEqual("cache/sccache", os.environ["SCCACHE_DIR"])
            self.assertEqual("config/sccache.toml", os.environ["SCCACHE_CONF"])
            self.assertEqual("logs/sccache.log", os.environ["SCCACHE_ERROR_LOG"])
            self.assertEqual("credentials/gcs.json", os.environ["SCCACHE_GCS_KEY_PATH"])
            self.dev._normalize_cache_paths("sccache", self.dev.Path("/workspace"))
            self.assertEqual("/workspace/cache/sccache", os.environ["SCCACHE_DIR"])
            self.assertEqual("/workspace/config/sccache.toml", os.environ["SCCACHE_CONF"])
            self.assertEqual("/workspace/logs/sccache.log", os.environ["SCCACHE_ERROR_LOG"])
            self.assertEqual(
                "/workspace/credentials/gcs.json",
                os.environ["SCCACHE_GCS_KEY_PATH"],
            )

    def test_absolute_cache_paths_are_preserved(self):
        with mock.patch.dict(
            os.environ, {"KACHE_CACHE_DIR": "/owned/cache"}, clear=True
        ):
            self.dev._normalize_cache_paths("kache", self.dev.Path("/elsewhere"))
            self.assertEqual("/owned/cache", os.environ["KACHE_CACHE_DIR"])

    def test_invalid_environment_backend_fails(self):
        with self.assertRaisesRegex(SystemExit, "invalid compiler cache"):
            self.configure(env={"PHOENIX_COMPILER_CACHE": "bogus"})


if __name__ == "__main__":
    unittest.main()
