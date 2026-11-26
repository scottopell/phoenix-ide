fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = std::path::PathBuf::from(args.next().ok_or("repository path")?);
    let open = || gix::open_opts(&path, gix::open::Options::isolated());
    let repo = open()?;
    let classify = |repo: &gix::Repository| -> Result<String, Box<dyn std::error::Error>> {
        let head = repo.head()?;
        match (head.referent_name(), head.id()) {
            (Some(name), Some(id)) => Ok(format!("attached {} {id}", name.as_bstr())),
            (None, Some(id)) => Ok(format!("detached {id}")),
            (Some(name), None) => Ok(format!("unborn {}", name.as_bstr())),
            (None, None) => Err("HEAD lacks both referent and object".into()),
        }
    };
    let before = classify(&repo)?;
    if let Some(next) = args.next() {
        let next = next.to_str().ok_or("non-UTF8 fixture ref")?;
        std::fs::write(path.join(".git/HEAD"), format!("ref: {next}\n"))?;
        let after = classify(&open()?)?;
        if before != after {
            return Err("HEAD changed during observation".into());
        }
    }
    println!("{before}");
    Ok(())
}
