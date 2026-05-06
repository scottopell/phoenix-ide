#[path = "../tls_certs.rs"]
mod tls_certs;

use std::{env, error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let command = args.next().ok_or_else(usage)?;
    let args: Vec<String> = args.collect();

    match command.as_str() {
        "ca" => cmd_ca(&args),
        "issue" => cmd_issue(&args),
        _ => Err(usage().into()),
    }
}

fn cmd_ca(args: &[String]) -> Result<(), Box<dyn Error>> {
    let dir = value_arg(args, "--dir")?;
    let ca = tls_certs::ensure_ca(&PathBuf::from(dir))?;
    println!("cert={}", ca.cert_path.display());
    println!("key={}", ca.key_path.display());
    Ok(())
}

fn cmd_issue(args: &[String]) -> Result<(), Box<dyn Error>> {
    let ca_dir = PathBuf::from(value_arg(args, "--ca-dir")?);
    let cert = PathBuf::from(value_arg(args, "--cert")?);
    let key = PathBuf::from(value_arg(args, "--key")?);
    let hosts = repeated_arg(args, "--host");
    if hosts.is_empty() {
        return Err("issue requires at least one --host".into());
    }

    let issued = tls_certs::issue_leaf(&ca_dir, &cert, &key, &hosts)?;
    println!("cert={}", issued.cert_path.display());
    println!("key={}", issued.key_path.display());
    println!("hosts={}", hosts.join(","));
    Ok(())
}

fn value_arg(args: &[String], name: &str) -> Result<String, Box<dyn Error>> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .ok_or_else(|| format!("missing required {name}").into())
}

fn repeated_arg(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .collect()
}

fn usage() -> String {
    "usage: phoenix-tls ca --dir DIR | phoenix-tls issue --ca-dir DIR --cert CERT --key KEY --host HOST [--host HOST ...]".into()
}
