use crate::DiagnosticPlan;
use std::{io::Read, path::Path};
pub fn run(
    plan: &DiagnosticPlan,
    output: &Path,
) -> Result<sil_instrument_console::diagnostics::RemoteDiagnosticReport, String> {
    let archive = if let Some(path) = plan.remote["archive"].as_str() {
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut bytes = vec![];
        f.take(65537)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 65536 {
            return Err("Archive reference exceeds limit".into());
        }
        Some(
            serde_json::from_value(
                yang_protocol::strict_json(&bytes, 65536).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(sil_instrument_console::diagnostics::diagnose_saved_peer(
            Path::new(plan.remote["peers"].as_str().unwrap()),
            plan.remote["host_id"].as_str().unwrap(),
            archive,
            output,
        ))
        .map_err(|e| e.to_string())
}
