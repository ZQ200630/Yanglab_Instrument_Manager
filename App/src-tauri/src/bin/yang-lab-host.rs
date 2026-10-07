use sil_instrument_console::host::{
    contracts::HostError,
    service::{HostConfig, HostService},
};
use std::path::PathBuf;
fn parse(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<HostConfig, HostError> {
    let mut root = None;
    let mut directory = None;
    let mut mode = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--resources") if root.is_none() => root = args.next().map(PathBuf::from),
            Some("--record-dir") if directory.is_none() => {
                directory = args.next().map(PathBuf::from)
            }
            Some("--real") if mode.is_none() => mode = Some("real".to_string()),
            _ => {
                return Err(HostError::new(
                    "HostArguments",
                    format!("Unknown or duplicate argument: {}", arg.to_string_lossy()),
                ))
            }
        }
    }
    let required = |name| HostError::new("HostArguments", format!("Missing {name}"));
    Ok(HostConfig {
        root: root.ok_or_else(|| required("--resources"))?,
        record_dir: directory.ok_or_else(|| required("--record-dir"))?,
        mode: mode.ok_or_else(|| required("--real"))?,
    })
}
fn run() -> Result<(), HostError> {
    let service = HostService::start(parse(std::env::args_os().skip(1))?)?;
    println!(
        "{}",
        serde_json::json!({"endpoint":service.endpoint(),"host":"Yang LAB INSTRUMENT CONSOLE"})
    );
    service.serve()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_worker_arguments_reject_legacy_selection_before_startup() {
        let parse_strings = |args: &[&str]| parse(args.iter().map(std::ffi::OsString::from));
        for args in [
            vec!["--python", "python.exe"],
            vec!["--root", "source"],
            vec!["--source-worker", "worker.py"],
            vec!["--simulate"],
            vec![
                "--resources",
                "one",
                "--resources",
                "two",
                "--record-dir",
                "record",
                "--real",
            ],
        ] {
            assert!(parse_strings(&args).is_err());
        }
        let config = parse_strings(&[
            "--resources",
            "C:/Yang 台子/native",
            "--record-dir",
            "C:/Yang 台子/config",
            "--real",
        ])
        .unwrap();
        assert_eq!(config.root, PathBuf::from("C:/Yang 台子/native"));
        assert_eq!(config.mode, "real");
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{}", serde_json::to_string(&error).unwrap());
        std::process::exit(2)
    }
}
