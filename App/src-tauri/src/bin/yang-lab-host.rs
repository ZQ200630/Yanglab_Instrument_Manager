use sil_instrument_console::host::{
    contracts::HostError,
    service::{HostConfig, HostService},
};
use std::path::PathBuf;
fn run() -> Result<(), HostError> {
    let mut args = std::env::args().skip(1);
    let mut root = None;
    let mut directory = None;
    let mut python = None;
    let mut mode = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--record-dir" => directory = args.next().map(PathBuf::from),
            "--python" => python = args.next().map(PathBuf::from),
            "--real" => mode = Some("real".to_string()),
            _ => {
                return Err(HostError::new(
                    "HostArguments",
                    format!("Unknown argument: {arg}"),
                ))
            }
        }
    }
    let required = |name| HostError::new("HostArguments", format!("Missing {name}"));
    let service = HostService::start(HostConfig {
        root: root.ok_or_else(|| required("--root"))?,
        record_dir: directory.ok_or_else(|| required("--record-dir"))?,
        python: python.ok_or_else(|| required("--python"))?,
        mode: mode.ok_or_else(|| required("--real"))?,
    })?;
    println!(
        "{}",
        serde_json::json!({"endpoint":service.endpoint(),"host":"Yang LAB INSTRUMENT CONSOLE"})
    );
    service.serve()
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{}", serde_json::to_string(&error).unwrap());
        std::process::exit(2)
    }
}
