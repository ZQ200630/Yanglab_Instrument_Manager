# CP210x / CP2102 Windows VCP driver

Unchanged Silicon Labs Universal Windows Driver 11.3.0 files, from its
official community attachment, downloaded on 2026-10-07:

- File page: https://community.silabs.com/s/contentversion/068Vm00000AX9rbIAD/detail?language=en_US
- Download: https://community.silabs.com/sfc/servlet.shepherd/version/download/068Vm00000AX9rbIAD
- ZIP SHA-256: `C4E9E7C631C1553B96539A19E58357FA0BEB04487ECFDD6E90B75AE783F08C46`
- Package version: `11.3.0`; release date: `2023-05-26`.

The vendor release notes explicitly list CP2102 and Windows 10 1803+ / Windows
11 x64. This is a pinned supported release, not the latest release: the current
main-site download was unavailable to this development session. No system
driver was changed or downgraded. Windows chooses driver rank during installation.

The original INF, CAT, four architecture SYS files, license and release notes
are bundled. CAT and SYS Authenticode signatures were Valid when inspected.
The package's unrelated UpdateParam.bat/UpdateParameters.reg are not bundled
or executed. Original license bytes are preserved; the same text is displayed
in the missing-driver wizard before its explicit Install driver acceptance.
The original license governs use and redistribution. The operator confirmed
manufacturer authorization to publish this unchanged package in this repository;
the original license and notices remain included.

The Host pins and locks every bundled file and elevates Windows System32 pnputil
for this fixed INF only. No caller paths, forced downgrade, registry patch or
automatic reboot are accepted. Installer completion alone is never device
readiness; a fresh Windows metadata check is required. Actual installation and
fresh-machine/hardware qualification were not performed.
