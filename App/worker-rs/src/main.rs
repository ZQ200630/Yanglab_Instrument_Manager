//! Supervised native worker. No environment-selected backend, interpreter or
//! diagnostic factory can be chosen by this production executable.
use std::path::PathBuf;
use yang_protocol::{valid_id, Limits};
use yang_worker::{
    backend::NativeBackend, captures::CaptureSpool, dispatch::Worker, scheduler::Scheduler,
    WorkerError,
};
struct Args {
    nonce: String,
    root: PathBuf,
}
fn args() -> Result<Args, WorkerError> {
    let mut real = false;
    let mut protocol = false;
    let mut nonce = None;
    let mut root = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--real") if !real=>real=true,
            Some("--protocol") if !protocol=> {if args.next().and_then(|v|v.into_string().ok()).as_deref()!=Some("3") {return Err(WorkerError::new("Arguments","Native worker requires protocol 3"));}protocol=true;},
            Some("--ownership-nonce") if nonce.is_none()=>nonce=args.next().and_then(|v|v.into_string().ok()),
            Some("--capture-spool") if root.is_none()=>root=args.next().map(PathBuf::from),
            _=>return Err(WorkerError::new("Arguments","Only supervised --real --protocol 3 --ownership-nonce --capture-spool are accepted")),
        }
    }
    let nonce = nonce
        .filter(|v| valid_id(v))
        .ok_or_else(|| WorkerError::new("Arguments", "Host-owned nonce required"))?;
    let root =
        root.ok_or_else(|| WorkerError::new("Arguments", "Host-owned capture staging required"))?;
    if !real || !protocol {
        return Err(WorkerError::new(
            "Arguments",
            "Native real protocol 3 worker required",
        ));
    }
    Ok(Args { nonce, root })
}
fn run() -> Result<(), WorkerError> {
    let args = args()?;
    let spool = CaptureSpool::open(args.root, args.nonce)?;
    let backend = NativeBackend::system()?;
    let scheduler = Scheduler::new(
        backend.clone(),
        backend.scheduler_clock(),
        Limits::default(),
    )?;
    let worker = Worker::new(backend, scheduler, spool);
    let mut receipt = worker.run_io(std::io::stdin(), std::io::stdout())?;
    if !receipt.all_resources_released {
        eprintln!(
            "Native responsibility retained; no force-kill or action replay. {}",
            serde_json::to_string(&receipt).unwrap_or_default()
        );
    }
    while !receipt.all_resources_released {
        std::thread::sleep(std::time::Duration::from_millis(2500));
        receipt = worker.retry_shutdown();
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
