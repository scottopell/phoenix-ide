use std::{io::Write, path::Path};

include!("capture-baseline.rs");

fn main() -> std::io::Result<()> {
    let path = std::env::args_os().nth(1).expect("disposable target reflog path");
    let bytes = read_bounded_regular_file_nofollow(Path::new(&path), 16 * 1024 * 1024)?;
    std::io::stdout().lock().write_all(&bytes)
}
