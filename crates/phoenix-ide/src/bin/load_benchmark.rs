use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let (mut db, mut fixtures, mut ui_dir, mut port) = (None, None, None, 8043);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(
            "usage: load_benchmark --db ABSOLUTE_DB --fixtures JSON [--port PORT] [--ui-dir DIR]",
        )?;
        match flag.as_str() {
            "--db" => db = Some(PathBuf::from(value)),
            "--fixtures" => fixtures = Some(PathBuf::from(value)),
            "--ui-dir" => ui_dir = Some(PathBuf::from(value)),
            "--port" => port = value.parse()?,
            _ => return Err(format!("unknown benchmark argument: {flag}").into()),
        }
    }
    let db = db.ok_or("--db is required")?;
    let fixtures = fixtures.ok_or("--fixtures is required")?;
    phoenix_ide::load_benchmark::serve(&db, &fixtures, port, ui_dir.as_deref()).await
}
