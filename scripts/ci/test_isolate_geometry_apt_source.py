"""Fixture-only tests: never read or mutate the host's apt sources."""

import contextlib
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import isolate_geometry_apt_source as subject


TARGET = subject.TARGET
OTHER = "https://archive.ubuntu.com/ubuntu"


def stanza(uris=TARGET, extra=""):
    return f"Types: deb deb-src\nURIs: {uris}\nSuites: noble\nComponents: main\n{extra}"


class SourceSyntaxTests(unittest.TestCase):
    def test_mixed_list_preserves_unrelated_lines_and_comments(self):
        before = f"# comment\r\ndeb {OTHER} noble main\r\n\r\n"
        after = f"  # deb {TARGET} noble main\r\ndeb https://example.org noble main"
        source = (before + f"deb [arch=amd64 signed-by=/usr/share/keyrings/ms.gpg] {TARGET} noble main # feed\r\n"
                  + f"deb-src {TARGET}/ noble main\r\n" + after)
        self.assertEqual(subject.isolate_list(source), (before + after, 2))

    def test_list_exact_path_suite_without_components(self):
        self.assertEqual(subject.isolate_list(f"deb {TARGET} ./\n"), ("", 1))

    def test_uri_near_matches_untouched_in_both_formats(self):
        for uri in (TARGET + "-extra", TARGET + "/extra", TARGET + "//", TARGET + "?q=1",
                    TARGET + "/?q=1", TARGET + "#fragment", TARGET.replace("https:", "http:"),
                    TARGET.replace("packages.", "user:secret@packages."),
                    TARGET.replace("packages.microsoft.com", "packages.microsoft.com.evil"),
                    TARGET.replace("24.04", "22.04"), TARGET.replace(".com/", ".com:443/")):
            with self.subTest(uri=uri):
                source = f"deb {uri} noble main\n"
                self.assertEqual(subject.isolate_list(source), (source, 0))
                source = stanza(uri)
                self.assertEqual(subject.isolate_sources(source), (source, 0))

    def test_list_malformed_target_fails_closed(self):
        for source in (f"deb {TARGET}\n", f"deb {TARGET} noble\n", f"rpm {TARGET} noble main\n",
                       f"deb [arch=amd64 {TARGET} noble main\n", f"deb [] {TARGET} noble main\n",
                       f"deb [bad-option] {TARGET} noble main\n", f"deb {OTHER} {TARGET} noble main\n",
                       f"deb {TARGET} noble main {OTHER}\n", f"deb {TARGET} ./ main\n"):
            with self.subTest(source=source), self.assertRaises(ValueError):
                subject.isolate_list(source)

    def test_deb822_mixed_stanzas_preserve_other_entries(self):
        before = "# Ubuntu\n" + stanza(OTHER) + "\n"
        target = stanza(TARGET + "/", "Signed-By: /usr/share/keyrings/ms.gpg\n")
        after = "\n# Another entry\n" + stanza("https://example.org")
        self.assertEqual(subject.isolate_sources(before + target + after),
                         (before + "Enabled: no\n" + target + after, 1))

    def test_deb822_mixed_uri_field_removes_only_exact_tokens(self):
        uris = f"{OTHER} {TARGET} {TARGET}/ {TARGET}/extra"
        source = stanza(uris, "Signed-By: /usr/share/keyrings/ms.gpg\n")
        expected = source.replace(f"{TARGET} {TARGET}/ ", "  ")
        self.assertEqual(subject.isolate_sources(source), (expected, 1))

    def test_deb822_multiline_uris_and_other_fields(self):
        source = (f"# Feed\nTypes: deb\n deb-src\nuRiS: {TARGET}\n"
                  f" {TARGET}/\n# between URIs\n\t{OTHER}\n {TARGET} https://example.org\n"
                  "Suites:\n noble\nComponents: main\n universe\nSigned-By:\n -----BEGIN PGP PUBLIC KEY BLOCK-----\n .\n abc\n -----END PGP PUBLIC KEY BLOCK-----\n")
        expected = source.replace(f"uRiS: {TARGET}\n", "uRiS: \n").replace(f" {TARGET}/\n", "")
        expected = expected.replace(f" {TARGET} https://example.org", "  https://example.org")
        self.assertEqual(subject.isolate_sources(source), (expected, 1))

    def test_deb822_only_target_folded_enabled(self):
        for enabled, disabled in (("Enabled: yes\n", "Enabled: no\n"),
                                  ("Enabled:\n yes\n", "Enabled:\n no\n")):
            with self.subTest(enabled=enabled):
                self.assertEqual(subject.isolate_sources(stanza(TARGET, enabled)),
                                 (stanza(TARGET, disabled), 1))

    def test_deb822_exact_path_without_components(self):
        source = stanza().replace("Suites: noble", "Suites: ./").replace("Components: main\n", "")
        self.assertEqual(subject.isolate_sources(source), ("Enabled: no\n" + source, 1))

    def test_deb822_crlf_and_no_final_newline(self):
        source = stanza().rstrip("\n").replace("\n", "\r\n")
        self.assertEqual(subject.isolate_sources(source), ("Enabled: no\r\n" + source, 1))

    def test_comments_disabled_and_noop_unchanged(self):
        for source in ("", "# comment\n\n", f"# URIs: {TARGET}\n", stanza(OTHER),
                       stanza(TARGET, "Enabled: no\n"),
                       f"Enabled: no\nURIs: {TARGET}\n"):
            with self.subTest(source=source):
                self.assertEqual(subject.isolate_sources(source), (source, 0))
        source = f"# deb {TARGET} noble main\n\t# deb {TARGET}/ noble main\n"
        self.assertEqual(subject.isolate_list(source), (source, 0))

    def test_deb822_malformed_target_shapes_fail_closed(self):
        cases = [stanza().replace("Types:", "Types"),
                 stanza().replace("Types: deb deb-src\n", ""),
                 stanza().replace("Types: deb deb-src", "Types: rpm"),
                 stanza().replace("Suites: noble\n", ""),
                 stanza().replace("Components: main\n", ""),
                 stanza(extra="Enabled: maybe\n"), stanza(extra="Enabled: yes no\n"),
                 stanza(extra="Enabled:\n"), stanza(extra="Enabled: \n \t\n"),
                 stanza().replace("Suites: noble", "Suites: ./"),
                 stanza(extra="URIs: https://example.org\n"),
                 stanza(extra="types: deb\n"), stanza(extra="Suites: stable\n"),
                 stanza(extra="Enabled: yes\nenabled: no\n"),
                 " orphan\n" + stanza(), stanza().replace("URIs:", "URI:"),
                 stanza(f"[{TARGET}]"), stanza(f'"{TARGET}"'),
                 stanza(f"{TARGET} not-a-uri"), stanza(f"{OTHER}\n URIs: {TARGET}"),
                 stanza(extra=f"Unknown: {TARGET}\n")]
        for source in cases:
            with self.subTest(source=source), self.assertRaises(ValueError):
                subject.isolate_sources(source)


class FilesystemTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.apt = self.root / "apt"
        self.apt.mkdir()
        (self.apt / "sources.list.d").mkdir()

    def write(self, relative, text):
        path = self.apt / relative
        path.write_bytes(text.encode())
        return path

    def test_enumerates_main_list_and_sources_not_other_extensions(self):
        main = self.write("sources.list", f"deb {TARGET} noble main\n")
        mixed = self.write("sources.list.d/arbitrary-name.list", f"deb {OTHER} noble main\ndeb {TARGET}/ noble main\n")
        folded = self.write("sources.list.d/another-name.sources", stanza(f"{OTHER} {TARGET}"))
        ignored = self.write("sources.list.d/keep.sources.save", stanza())
        nested = self.apt / "sources.list.d" / "nested"
        nested.mkdir()
        (nested / "keep.list").write_text(f"deb {TARGET} noble main\n")
        main.chmod(0o640)
        self.assertEqual(subject.isolate(self.apt), (3, 3, 3))
        self.assertEqual(main.read_bytes(), b"")
        self.assertEqual(main.stat().st_mode & 0o777, 0o640)
        self.assertEqual(mixed.read_text(), f"deb {OTHER} noble main\n")
        self.assertEqual(folded.read_text(), stanza(OTHER + " "))
        self.assertEqual(ignored.read_text(), stanza())
        self.assertEqual(subject.isolate(self.apt), (3, 0, 0))

    def test_missing_main_and_empty_parts_is_noop(self):
        self.assertEqual(subject.isolate(self.apt), (0, 0, 0))
        (self.apt / "sources.list.d").rmdir()
        self.assertEqual(subject.isolate(self.apt), (0, 0, 0))

    def test_malformed_late_file_never_partially_edits_earlier_files(self):
        for suffix, malformed in (("list", f"deb {TARGET} noble\n"),
                                  ("sources", stanza(extra="URIs: duplicate\n")),
                                  ("sources", stanza().replace("Suites:", "Bad field:"))):
            with self.subTest(suffix=suffix, malformed=malformed):
                good = self.write("sources.list", f"deb {TARGET} noble main\n")
                bad = self.write("sources.list.d/z." + suffix, malformed)
                snapshot = {path: path.read_bytes() for path in (good, bad)}
                with self.assertRaises(ValueError):
                    subject.isolate(self.apt)
                self.assertEqual({path: path.read_bytes() for path in (good, bad)}, snapshot)
                bad.unlink()

    def test_invalid_utf8_late_file_does_not_edit_earlier_files(self):
        good = self.write("sources.list", f"deb {TARGET} noble main\n")
        bad = self.apt / "sources.list.d/z.sources"
        bad.write_bytes(b"\xff")
        with self.assertRaises(UnicodeError):
            subject.isolate(self.apt)
        self.assertEqual(good.read_text(), f"deb {TARGET} noble main\n")
        self.assertEqual(bad.read_bytes(), b"\xff")

    def test_malformed_late_stanza_does_not_edit_same_file(self):
        source = stanza() + "\n" + stanza(extra="URIs: duplicate\n")
        path = self.write("sources.list.d/mixed.sources", source)
        with self.assertRaises(ValueError):
            subject.isolate(self.apt)
        self.assertEqual(path.read_text(), source)

    def test_source_symlink_is_rejected_without_touching_outside_file(self):
        outside = self.root / "outside"
        outside.write_text(stanza())
        good = self.write("sources.list", f"deb {TARGET} noble main\n")
        linked = self.apt / "sources.list.d/z.sources"
        linked.symlink_to(outside)
        with self.assertRaises(ValueError):
            subject.isolate(self.apt)
        self.assertTrue(linked.is_symlink())
        self.assertEqual(outside.read_text(), stanza())
        self.assertEqual(good.read_text(), f"deb {TARGET} noble main\n")

    def test_dangling_main_symlink_is_rejected(self):
        (self.apt / "sources.list").symlink_to(self.root / "missing")
        with self.assertRaises(ValueError):
            subject.isolate(self.apt)
        self.assertFalse((self.root / "missing").exists())

    def test_source_directory_symlink_and_root_alias_are_rejected(self):
        parts = self.apt / "sources.list.d"
        parts.rmdir()
        outside = self.root / "outside"
        outside.mkdir()
        target = outside / "source.sources"
        target.write_text(stanza())
        parts.symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            subject.isolate(self.apt)
        self.assertEqual(target.read_text(), stanza())
        alias = self.root / "alias"
        alias.symlink_to(self.apt, target_is_directory=True)
        with self.assertRaises(ValueError):
            subject.isolate(alias)

    def test_hardlink_or_nonregular_candidate_is_rejected(self):
        original = self.root / "original"
        original.write_text(stanza())
        linked = self.apt / "sources.list.d/linked.sources"
        os.link(original, linked)
        with self.assertRaises(ValueError):
            subject.isolate(self.apt)
        self.assertEqual(original.read_text(), stanza())
        linked.unlink()
        linked.mkdir()
        with self.assertRaises(ValueError):
            subject.isolate(self.apt)

    def test_cli_logs_counts_only_or_fixed_error(self):
        secret = "https://user:secret@example.org/"
        self.write("sources.list", f"deb {secret} noble main\ndeb {TARGET} noble main\n")
        counts = subject.isolate(self.apt)
        output = io.StringIO()
        with mock.patch.object(subject, "isolate", return_value=counts) as isolate:
            with contextlib.redirect_stdout(output):
                self.assertEqual(subject.main(), 0)
            isolate.assert_called_once_with(Path("/etc/apt"))
        self.assertEqual(output.getvalue(), "Geometry apt sources: scanned=1 edited=1 isolated_entries=1\n")
        output = io.StringIO()
        with mock.patch.object(subject, "isolate", side_effect=OSError(secret)):
            with contextlib.redirect_stderr(output):
                self.assertEqual(subject.main(), 1)
        self.assertEqual(output.getvalue(), "Geometry apt source isolation failed; no source contents logged.\n")

    def test_workflow_keeps_full_browser_install_after_helper(self):
        workflow = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"
        geometry = workflow.read_text().split("  check-finalization-geometry:\n", 1)[1].split("\n  check-fast:", 1)[0]
        self.assertIn("          sudo python3 ../scripts/ci/isolate_geometry_apt_source.py\n"
                      "          pnpm exec playwright install --with-deps chromium\n", geometry)
        self.assertIn("python3 -m unittest discover -s ../scripts/ci -p 'test_isolate_geometry_apt_source.py'", geometry)


if __name__ == "__main__":
    unittest.main()
