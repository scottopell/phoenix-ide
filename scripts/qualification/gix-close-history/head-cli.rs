use std::{
    path::Path,
    process::{Command, Output},
    time::{Duration, Instant},
};

include!("baseline.rs");
include!("head-baseline.rs");

type WorktreeGitLeaf<'a> = dyn FnMut(Command, Instant, &str) -> Result<Output, String> + 'a;

fn run_bounded_git_command_with_leaf_until(
    repository: &Path,
    args: &[&str],
    _input: Option<&[u8]>,
    deadline: Instant,
    operation: &str,
    leaf: &mut WorktreeGitLeaf<'_>,
) -> Result<Output, String> {
    let mut command = Command::new("git");
    command.current_dir(repository).args(args);
    leaf(command, deadline, operation)
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let directory = args.next().ok_or("repository path")?;
    let changed_to = args.next();
    let inspection = inspect_head_with_leaf_until(
        Path::new(&directory),
        Instant::now() + Duration::from_secs(5),
        "HEAD fixture",
        &mut |mut command, _, _| {
            let output = command.output().map_err(|err| err.to_string())?;
            if command.get_args().next().is_some_and(|arg| arg == "show-ref")
                && let Some(next) = &changed_to
            {
                std::fs::write(Path::new(&directory).join(".git/HEAD"), format!("ref: {next}\n"))
                    .map_err(|err| err.to_string())?;
            }
            Ok(output)
        },
    )?;
    let text = |bytes: Vec<u8>| String::from_utf8(bytes).map_err(|err| err.to_string());
    match inspection {
        InspectedHead::Resolved { symbolic: Some(name), oid } => println!("attached {} {}", text(name)?, text(oid)?),
        InspectedHead::Resolved { symbolic: None, oid } => println!("detached {}", text(oid)?),
        InspectedHead::Unborn { symbolic } => println!("unborn {}", text(symbolic)?),
    }
    Ok(())
}
