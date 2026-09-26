import json
import shutil
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RULE = ROOT / "ast-grep-rules" / "rust-no-manual-sql-transactions.yml"


@unittest.skipUnless(shutil.which("ast-grep"), "ast-grep is not installed")
class ManualSqlTransactionTests(unittest.TestCase):
    def findings(self, source):
        result = subprocess.run(
            ["ast-grep", "scan", "--rule", str(RULE), "--stdin", "--json=compact"],
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


if __name__ == "__main__":
    unittest.main()
