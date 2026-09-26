import json
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CHECKER = ROOT / "scripts" / "check_manual_sql_transactions.py"


@unittest.skipUnless(shutil.which("ast-grep"), "ast-grep is not installed")
class ManualSqlTransactionTests(unittest.TestCase):
    def findings(self, source):
        result = subprocess.run(
            [sys.executable, str(CHECKER), "--stdin", "--json"],
            input=source, text=True, capture_output=True, check=False,
        )
        self.assertIn(result.returncode, (0, 1), result.stderr)
        return json.loads(result.stdout)

    def test_rejects_manual_transaction_controls(self):
        for control in ("BEGIN", "BEGIN IMMEDIATE", "COMMIT", "END", "ROLLBACK",
                        "ROLLBACK TO partial", "SAVEPOINT partial", "RELEASE partial"):
            for call in ('sqlx::query("%s")', 'sqlx::raw_sql("%s")',
                         'query("%s")', 'sqlx::query::<sqlx::Sqlite>("%s")',
                         'connection.execute("%s")',
                         'connection.execute_batch("%s")'):
                with self.subTest(control=control, call=call):
                    source = 'async fn production() { ' + call % control + '; }'
                    self.assertEqual(1, len(self.findings(source)))

    def test_raw_multiline_and_case_insensitive_sql(self):
        source = '''async fn production() {
            sqlx::raw_sql(r#"
                bEgIn immediate;
            "#);
            connection.execute("  rollback");
        }'''
        self.assertEqual(2, len(self.findings(source)))

    def test_leading_comments_do_not_hide_transaction_control(self):
        source = r'''async fn production() {
            sqlx::query("/* reservation */ BEGIN IMMEDIATE");
            connection.execute("\n ROLLBACK");
            sqlx::raw_sql(r#"-- reservation
                BEGIN IMMEDIATE"#);
        }'''
        self.assertEqual(3, len(self.findings(source)))

    def test_owned_transactions_and_migration_triggers_are_allowed(self):
        source = '''async fn production() {
            let tx = pool.begin_with("BEGIN IMMEDIATE").await;
            let tx = connection.begin().await;
            tx.commit().await;
            tx.rollback().await;
            sqlx::raw_sql("CREATE TRIGGER guard BEFORE INSERT ON rows
                BEGIN SELECT RAISE(ABORT, 'invalid'); END");
            connection.execute("SELECT 'BEGIN'");
        }'''
        self.assertEqual([], self.findings(source))

    def test_escaped_newlines_terminate_normal_string_sql_comments(self):
        for sql in (r'-- reservation\nBEGIN IMMEDIATE',
                    r'-- reservation\r\nROLLBACK',
                    r'-- first\n-- second\nCOMMIT',
                    r'-- backslash \\\nSAVEPOINT partial'):
            with self.subTest(sql=sql):
                source = 'async fn production() { connection.execute("' + sql + '"); }'
                self.assertEqual(1, len(self.findings(source)))

    def test_literal_backslashes_do_not_terminate_sql_comments(self):
        source = r'''async fn production() {
            connection.execute("-- reservation\\nBEGIN IMMEDIATE");
            connection.execute(r#"-- reservation\nBEGIN IMMEDIATE"#);
            connection.execute(r#"\nBEGIN IMMEDIATE"#);
        }'''
        self.assertEqual([], self.findings(source))

    def test_cfg_test_module_and_helpers_are_exempt(self):
        source = '''
        #[cfg(test)]
        mod tests {
            async fn helper() { connection.execute("BEGIN"); }
            mod nested { fn fixture() { sqlx::query("ROLLBACK"); } }
        }
        #[cfg(test)]
        #[allow(dead_code)]
        async fn fixture() { sqlx::query("COMMIT"); }
        #[tokio::test]
        async fn standalone_test() { connection.execute("BEGIN"); }
        '''
        self.assertEqual([], self.findings(source))

    def test_test_exemption_cannot_leak_into_production(self):
        source = '''
        #[cfg(test)]
        mod tests { fn fixture() { sqlx::query("BEGIN"); } }
        async fn production() { sqlx::query("BEGIN"); }
        #[cfg(not(test))]
        async fn also_production() { connection.execute("COMMIT"); }
        #[cfg(any(test, feature = "production"))]
        async fn shared() { sqlx::raw_sql("ROLLBACK"); }
        '''
        self.assertEqual(3, len(self.findings(source)))

    def test_every_statement_is_checked(self):
        source = r'''async fn production() {
            sqlx::raw_sql("PRAGMA foreign_keys = ON; BEGIN IMMEDIATE");
            connection.execute_batch("SELECT 1; -- comment\nSAVEPOINT partial; RELEASE partial;");
            connection.execute("; ; COMMIT;");
            sqlx::raw_sql("CREATE TRIGGER guard BEFORE INSERT ON rows
                BEGIN SELECT 1; SELECT 2; END; BEGIN IMMEDIATE;");
        }'''
        self.assertEqual(4, len(self.findings(source)))

    def test_quoted_and_commented_semicolons_are_not_boundaries(self):
        source = r'''async fn production() {
            sqlx::raw_sql("SELECT '; BEGIN', 'it''s; ROLLBACK', \"; END\", [; RELEASE], `; SAVEPOINT`;");
            connection.execute_batch("SELECT 1 /* ; BEGIN */; -- ; COMMIT\nSELECT 2;");
            sqlx::raw_sql("CREATE TRIGGER guard BEFORE INSERT ON rows BEGIN
                SELECT CASE WHEN 1 THEN 'a; BEGIN' ELSE 'b' END; SELECT 2; END;");
        }'''
        self.assertEqual([], self.findings(source))

    def test_rust_escapes_are_decoded_before_sql_inspection(self):
        source = r'''async fn production() {
            connection.execute("-- comment\x0aBEGIN IMMEDIATE");
            connection.execute("-- comment\u{0_00a}COMMIT");
            connection.execute("\x42EGIN IMMEDIATE");
            connection.execute("\
                ROLLBACK");
        }'''
        self.assertEqual(4, len(self.findings(source)))

    def test_executor_and_bound_argument_calls_are_checked(self):
        source = '''async fn production() {
            sqlx::Executor::execute(&mut connection, "BEGIN");
            sqlx::query_with("COMMIT", arguments);
            connection.fetch_all("ROLLBACK");
        }'''
        self.assertEqual(3, len(self.findings(source)))

    def test_function_spelling_does_not_hide_literal_calls(self):
        for function in ("sqlx :: query", "connection . execute", "sqlx::r#query",
                         "connection.r#execute", "sqlx::query :: <sqlx::Sqlite>",
                         "sqlx::query::<\nsqlx::Sqlite\n>"):
            with self.subTest(function=function):
                source = 'async fn production() { ' + function + '("BEGIN"); }'
                self.assertEqual(1, len(self.findings(source)))

    def test_parentheses_preserve_literal_argument_identity(self):
        source = '''async fn production() {
            connection.execute((("BEGIN")));
            sqlx::query_with("SELECT ?1", ("COMMIT",));
            sqlx::query_with("SELECT ?1", ["ROLLBACK"]);
            sqlx::query("SELECT ?1").bind("SAVEPOINT partial");
        }'''
        self.assertEqual(1, len(self.findings(source)))

    def test_sqlite_bom_is_whitespace(self):
        source = r'''async fn production() {
            connection.execute("\u{feff}BEGIN");
            sqlx::raw_sql("SELECT 1; /* boundary */\u{feff}COMMIT;");
        }'''
        self.assertEqual(2, len(self.findings(source)))


if __name__ == "__main__":
    unittest.main()
