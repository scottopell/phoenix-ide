from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "check_migration_ledger.py"

spec = importlib.util.spec_from_file_location("check_migration_ledger", SCRIPT)
assert spec and spec.loader
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


def ledger(*entries: str) -> str:
    return "const MIGRATIONS: &[Migration] = &[\n" + "\n".join(entries) + "\n];\n"


def entry(version: int, name: str, sql: str | None = None) -> str:
    sql_value = sql or f"MIGRATION_{version:03d}"
    return (
        "    Migration {\n"
        f"        version: {version},\n"
        f'        name: "{name}",\n'
        f"        sql: {sql_value},\n"
        "    },"
    )


class MigrationLedgerCheckTests(unittest.TestCase):
    def test_current_ledger_is_structurally_valid(self):
        source = checker.DEFAULT_LEDGER.read_text()
        self.assertEqual([], checker.validate_migration_ledger(source))

    def test_rejects_duplicate_and_out_of_order_versions(self):
        source = ledger(entry(2, "second"), entry(2, "duplicate"), entry(1, "first"))
        errors = checker.validate_migration_ledger(source)
        self.assertTrue(any("strictly increasing" in error for error in errors))
        self.assertTrue(any("duplicate migration versions" in error for error in errors))

    def test_comments_and_entry_text_inside_strings_are_ignored(self):
        source = ledger(
            """    Migration {
        version: 1,
        // Migration { version: 99 is not a ledger entry.
        name: "first",
        sql: "Migration { version: 99 }",
    },""",
            entry(2, "second"),
        )
        self.assertEqual([], checker.validate_migration_ledger(source))

    def test_array_delimiter_inside_sql_does_not_hide_invalid_tail(self):
        source = ledger(
            """    Migration {
        version: 2,
        name: "second",
        sql: "line one
];
line three",
    },""",
            entry(1, "out_of_order"),
        )
        self.assertTrue(
            any(
                "strictly increasing" in error
                for error in checker.validate_migration_ledger(source)
            )
        )

    def test_valid_char_literals_lifetimes_and_unicode_outside_ledger_are_ignored(self):
        source = ledger(entry(1, "first")) + r'''
const QUOTE: char = '"';
const BYTE_QUOTE: u8 = b'"';
const ESCAPED_QUOTE: char = '\'';
const BACKSLASH: char = '\\';
fn identity<'a>(café: &'a str) -> &'a str { café }
#[test] fn raw_c_string() { let _ = cr#"quote ""#; }
const MIGRATIONŚ: u32 = 1;
'''
        self.assertEqual([], checker.validate_migration_ledger(source))

    def test_rejects_a_nonliteral_migrations_initializer(self):
        source = """const MIGRATIONS: &[Migration] = if false {
    &[Migration { version: 1, name: \"first\", sql: MIGRATION_001 }]
} else {
    &[Migration { version: 1, name: \"first\", sql: MIGRATION_001 }]
};
"""
        self.assertIn(
            "MIGRATIONS declaration is not a literal borrowed Migration array",
            checker.validate_migration_ledger(source),
        )

    def test_rejects_a_decoy_array_inside_the_declared_type(self):
        source = """const MIGRATIONS: &[Migration; {
    let _ = &[Migration { version: 1, name: \"decoy\", sql: MIGRATION_001 }];
    2
}] = &[
    Migration { version: 1, name: \"first\", sql: MIGRATION_001 },
    Migration { version: 1, name: \"duplicate\", sql: MIGRATION_001 },
];
"""
        self.assertIn(
            "MIGRATIONS declaration is not a literal borrowed Migration array",
            checker.validate_migration_ledger(source),
        )

    def test_rejects_an_unparsed_array_element(self):
        source = ledger(entry(1, "first"), "    DUPLICATE,")
        self.assertIn(
            "ledger element 2 is not a literal Migration entry",
            checker.validate_migration_ledger(source),
        )

    def test_rejects_a_version_expression_instead_of_reading_its_prefix(self):
        expression = entry(2, "second").replace("version: 2", "version: 2 - 1")
        self.assertIn(
            "entry 2 has no literal decimal version",
            checker.validate_migration_ledger(ledger(entry(1, "first"), expression)),
        )

    def test_underlined_decimal_version_uses_its_complete_value(self):
        underlined = entry(10, "tenth").replace("version: 10", "version: 1_0")
        self.assertEqual([], checker.validate_migration_ledger(ledger(underlined)))
        mismatch = underlined.replace("MIGRATION_010", "MIGRATION_001")
        self.assertIn(
            "migration 10 references MIGRATION_001",
            checker.validate_migration_ledger(ledger(mismatch)),
        )

    def test_qualified_numbered_sql_reference_is_checked(self):
        qualified = entry(1, "first", "self::MIGRATION_002")
        self.assertIn(
            "migration 1 references MIGRATION_002",
            checker.validate_migration_ledger(ledger(qualified)),
        )

    def test_rejects_duplicate_names(self):
        source = ledger(entry(1, "same"), entry(2, "same"))
        self.assertTrue(
            any(
                "duplicate migration names" in error
                for error in checker.validate_migration_ledger(source)
            )
        )

    def test_rejects_mismatched_numbered_sql_constant(self):
        source = ledger(entry(7, "seventh", "MIGRATION_008"))
        self.assertIn(
            "migration 7 references MIGRATION_008",
            checker.validate_migration_ledger(source),
        )

    def test_rejects_a_non_snake_case_name(self):
        source = ledger(entry(1, "valid"), entry(2, "Not-Snake-Case"))
        self.assertIn(
            "migration 2 has no snake-case literal name",
            checker.validate_migration_ledger(source),
        )


if __name__ == "__main__":
    unittest.main()
