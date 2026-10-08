# CH340 / CH341 Windows VCP driver

The unchanged Windows 10/11 files are from WCH's CH341SER.ZIP 4.0 package,
downloaded via the official page API on 2026-10-07:

- Page: https://www.wch.cn/downloads/CH341SER_ZIP.html
- Download: https://file.wch.cn/download/file?id=5
- ZIP SHA-256: `59967D9CE371D0BF3DF02DEC0B66C8DFBF9CA576DA0572FF4404148A7C381807`
- INF DriverVer: `02/11/2026, 4.0.2026.02`

The nine files in the ZIP's `CH341SER/WIN 1X/` folder are included unchanged.
Their CAT, SYS and DLL Authenticode signatures were Valid when inspected.
The official download page explicitly describes this package as suitable for
integration into product applications. The app's fixed, compiled package
manifest pins every included file's size and SHA-256.

No driver is installed during app startup or app installation. A driverless,
present matching USB device (Windows problem 28) exposes Install driver in
Add New Instrument and an explicit Install action in Settings. The Host verifies
and locks all package files, then
elevates Windows System32 pnputil for this fixed INF, with no forced downgrade
or automatic reboot. All instrument sessions must first be released.
Installation and fresh-machine/hardware qualification were not performed.
