use phoenix_deployment_controller::{ControllerSqliteStore, ControllerStoreError};
use rusqlite::{types::Value, Connection};

type SchemaObjects = Vec<(String, String)>;
type TableRows = Vec<(String, Vec<Vec<Value>>)>;
type SchemaSnapshot = (i64, SchemaObjects, TableRows);

fn snapshot(connection: &Connection) -> SchemaSnapshot {
    let version = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let schema: Vec<(String, String)> = connection
        .prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut data = Vec::new();
    for (table, _) in schema
        .iter()
        .filter(|(_, sql)| sql.starts_with("CREATE TABLE"))
    {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|column| row.get(column))
                    .collect::<Result<Vec<Value>, _>>()
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        data.push((table.clone(), rows));
    }
    (version, schema, data)
}

type StoreOpener = fn(&std::path::Path) -> Result<ControllerSqliteStore, ControllerStoreError>;

fn open_store(path: &std::path::Path) -> Result<ControllerSqliteStore, ControllerStoreError> {
    ControllerSqliteStore::open(path)
}

fn open_existing_store(
    path: &std::path::Path,
) -> Result<ControllerSqliteStore, ControllerStoreError> {
    ControllerSqliteStore::open_existing(path)
}

fn open_read_only_store(
    path: &std::path::Path,
) -> Result<ControllerSqliteStore, ControllerStoreError> {
    ControllerSqliteStore::open_read_only(path)
}

#[test]
fn prior_v5_schema_and_normalized_rows_are_untouched_by_every_open_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v5.sqlite");
    let raw = Connection::open(&path).unwrap();
    raw.pragma_update(None, "foreign_keys", true).unwrap();
    raw.execute_batch(include_str!("fixtures/controller_v5.sql"))
        .unwrap();
    assert_eq!(
        raw.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    let before = snapshot(&raw);
    assert_eq!(before.0, 5);
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM native_sampling_ack_confirmations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM native_reconciliation_activations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let openers: [StoreOpener; 3] = [open_store, open_existing_store, open_read_only_store];
    for open in openers {
        assert!(matches!(
            open(&path),
            Err(ControllerStoreError::UnsupportedSchema(5))
        ));
        assert_eq!(snapshot(&raw), before);
    }
}

#[test]
fn fresh_v6_has_normalized_child_tables_and_no_prior_rows() {
    let store = ControllerSqliteStore::in_memory().unwrap();
    drop(store);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v6.sqlite");
    drop(ControllerSqliteStore::open(&path).unwrap());
    let raw = Connection::open(&path).unwrap();
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
    for table in [
        "native_child_list_issues",
        "native_child_list_applications",
        "native_child_list_result_children",
        "native_child_list_ack_confirmations",
    ] {
        assert_eq!(
            raw.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
