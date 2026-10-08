# Independent asynchronous request validation

Baseline: `f206ca3894eba025c2b05856fc9259a494f10920`. The user wants asynchronous operations and a responsive interface. Existing asynchronous frontend and actor interfaces are retained. This change removes two avoidable communication queues: long background requests shared the control pipe, and status queries shared the file/result pipe.

## Evidence

- RED: routing put `driver_status` on control; the actual prior Host rejected new status attachment with `SessionAttach: Invalid channel`. GREEN: the GUI routes inventory, scan, connection test, device refresh and driver installation to background; lightweight operation/snapshot/worker/catalog/install-status queries use status; file replies remain on results.
- Finite named-pipe peers hold a background connection test and an archive read. A real `LocalHostClient` control prepare and status operation query both return within a 300 ms bound while the original replies remain held. Original finite work is then released and settled. This establishes queue independence, not hardware execution speed.
- Seven channel names authenticate to one logical session. Foreign tokens and duplicates are rejected, with the existing limit of sixteen logical clients preserved. Real-Host process fixtures verify that status cannot execute, acquire control or install, and background cannot execute or read a general snapshot. Status closure preserves the session; background closure revokes it. Old clients retain their existing channel permissions.
- RED: a call already waiting on the pipe mutex could send after that transport was poisoned. GREEN: the failure flag is checked again after acquiring the mutex; the finite server observes no request frame.
- A finite old-Host peer rejects background/status attachment. The GUI receives a clear upgrade/restart error, and the peer observes no replay or fallback request.
- Final Rust library suite: **175/175**. Offline App Python suite: **505/505** using the Anaconda VISA executable and injected bounded worker transports. The parser error printed by a negative worker-argument case is expected; the suite exits successfully.
- Existing Node suite: **251/251**. Isolated Edge frontend acceptance passes, including navigation and visible progress during a delayed read. No frontend changes or dependencies were needed for this task.
- One independent read-only review examined all six changed code/test files and the plan. No Critical, Important or useful Minor findings; no fix pass was needed. The reviewer did not rerun process tests or access hardware.

Commands and logs are under ignored `Result/async-requests`. Source and runtime tests do not open instruments. All Python commands use `D:/Program/Anaconda3/envs/VISA/python.exe`.

## Boundaries

One pipe still serializes complete framed request/reply exchanges. The Host admission mutex, shared SDK/query gates, per-device command ordering, driver installation fencing, lease and cleanup responsibilities are unchanged. A shared hardware resource can still require a wait. Separate communication channels do not authorize concurrent writes to the same device, retries, or cancellation of hardware work.

No laser scan, connection diagnostic, output action, tuning or installation was run in this task. Validation uses finite transports and read-only Host startup metadata. The previous registered TLB domain was DISCONNECTED and its control AVAILABLE before the GUI was normally closed. A subsequent authenticated Host stop separately confirmed resource release and successful worker process exit; the Host process then exited normally. No real GUI, Host or native hardware process was force-killed.

## Delivery

Host and GUI offline debug builds succeeded, with three preexisting GUI Rust warnings. Package verification confirmed **40** resources match source and no retired resources are installed. GUI SHA256: `FDC338F681DB2516ED82AF26F9CEF040999150CFC493DDFF780EACBA10D29536`. Host SHA256: `2853C657E302AB746CED02C34AE79E2B115F7072AD3A9BAF567D60D11FF79152`.

The rebuilt GUI (PID 20512) and Host (PID 53200) were responding. An authenticated observer attached background and status to the same logical session and queried the snapshot through status: startup was healthy, the registered TLB domain remained DISCONNECTED and its control AVAILABLE. The observer explicitly closed its own session. Exactly one GUI and Host were present; no native TLB worker was running. Historical unresolved ownership evidence retained its original SHA256. No native desktop interaction timing or real-device throughput is claimed by these startup checks.
