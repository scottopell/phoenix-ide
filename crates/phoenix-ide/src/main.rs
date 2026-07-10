#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    phoenix_ide::run_server().await
}
