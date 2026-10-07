fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = (|| -> Result<bool, String> {
        let plan = yang_debug::parse_args(&args)?;
        if !plan.execute_requested() {
            println!("No resources opened. Preview only.\nCommands: enumerate osa voltage gain pm400 mdt fiber remote\nUse --stage <enumerate|readonly|action> --out <absolute Result/name> and typed --binding JSON. Execute only after separate operator authorization: --execute --confirm-stage <stage>. Gain/Voltage action startup/close require --acknowledge-lifecycle. Remote uses existing --peers DPAPI file --host ID; optional --archive reference.json.");
            println!("{}", serde_json::to_string_pretty(&plan.preview()).unwrap());
            return Ok(true);
        }
        eprintln!("Diagnostic pending. No operation is automatically retried.");
        let auth = yang_debug::DiagnosticAuthorization::from_plan(&plan)?;
        let report = yang_debug::execute(plan, auth)?;
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        Ok(report.success())
    })();
    let mut code = match result {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("{e}");
            2
        }
    };
    while yang_debug::retry_pending() != 0 {
        eprintln!("Native resource release remains pending. Safety owner retained; do not force terminate.");
        std::thread::sleep(std::time::Duration::from_secs(1));
        code = 1;
    }
    std::process::exit(code);
}
