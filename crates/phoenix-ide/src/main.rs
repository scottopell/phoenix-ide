#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let migrate_only = {
        let mut args = std::env::args().skip(1);
        matches!(args.next().as_deref(), Some("--migrate-only")) && args.next().is_none()
    };
    if migrate_only {
        phoenix_ide::migrate_database().await
    } else {
        phoenix_ide::run_server().await
    }
}
