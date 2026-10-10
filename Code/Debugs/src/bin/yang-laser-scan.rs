fn main() {
    let result=(||->Result<(),String>{
        let args:Vec<_>=std::env::args().skip(1).collect();
        if args.first().is_some_and(|arg|arg=="--review") {
            if args.len()!=7&&args.len()!=9||args.len()==9&&args[7]!="--candidate" {return Err("Use --review <six report files> [--candidate <absolute new Result/name/profile.json>]; review never opens hardware or installs a profile".into());}
            let paths=args[1..7].iter().map(std::path::PathBuf::from).collect::<Vec<_>>();
            let candidate=yang_debug::laser_scan::review_candidate(&paths)?;
            if args.len()==9 {yang_debug::laser_scan::write_candidate(std::path::Path::new(&args[8]),&candidate)?;}
            println!("{}",serde_json::to_string_pretty(&candidate).map_err(|e|e.to_string())?);
            return Ok(());
        }
        if args.len()!=2&&args.len()!=5 {return Err("Use --plan <JSON-file> for preview; append --execute --confirm-stage <enumerate|readonly|action> only after separate stage authorization".into());}
        if args[0]!="--plan"||(args.len()==5&&(args[2]!="--execute"||args[3]!="--confirm-stage")) {return Err("Unknown or missing diagnostic arguments".into());}
        let path=std::path::Path::new(&args[1]);
        if std::fs::metadata(path).map_err(|e|e.to_string())?.len()>16384 {return Err("Plan exceeds capacity".into());}
        let plan=yang_debug::laser_scan::parse_plan(&std::fs::read(path).map_err(|e|e.to_string())?)?;
        if args.len()==2 {println!("{}",serde_json::to_string_pretty(&yang_debug::laser_scan::preview(&plan)).unwrap());return Ok(());}
        #[cfg(windows)] let report=yang_debug::laser_scan::execute(&plan,&args[4])?;
        #[cfg(not(windows))] return Err("Newport diagnostic requires Windows".into());
        #[cfg(windows)] {
            println!("{}",serde_json::to_string_pretty(&report).unwrap());
            if !report["error"].is_null()||report["resources_released"]!=true {return Err("Diagnostic incomplete; inspect immutable report".into());}
        }
        Ok(())
    })();
    if let Err(error)=result {eprintln!("{error}");std::process::exit(1);}
}
