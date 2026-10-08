# Newport USB package

`USBDriverSetup64.msi` is the x64 package from the operator's downloaded **Newport USB Driver 5.0.8** archive, obtained from [Newport downloads](https://download.newport.com/#/Software/Newport_USB_Driver). It is included unchanged.

- Size: 14,447,104 bytes.
- SHA-256: `8F8FAFEDF3ADB94F922E6A1BD7CD14E5F8AEE2F3B0F501A8FC2348927ADFDB19`.
- Downloaded ZIP SHA-256: `2F68A5C10DE10799DF94C14F67EFCFE3C15EC44B49A91CF69B9E4F9AC7E7AA9F`.
- Windows Authenticode inspection of these MSI bytes reports **NotSigned**. Hash checking pins these downloaded bytes; it does not establish a publisher signature.

The native Host accepts only the fixed `newport` package identifier. It checks the exact length and digest, keeps the file protected against write/delete, launches Windows' system msiexec with UAC, and waits for the original process. Installation requires all instrument resources and global SDK handles to be released. Cancelled, failed, reboot-required and unknown exit states are never reported as driver readiness. An unconfirmed exit blocks new connections and installation until Windows recovery.

The package is bundled, not installed at App startup. Installation requires the operator's explicit Install driver click and Windows elevation. Offline verification checks bytes and admission behavior; it does not execute the vendor installer or qualify driver installation on a fresh machine.

CH340 and CP210x packages now use the same fixed-package installation admission.
All three install actions require a missing-driver check. Add New Instrument
provides its connection prerequisite action; Settings also offers an explicit
Install button beside a missing package. Startup never prompts or installs.
The production App now uses the linked native Rust Worker. Historical Python source is excluded from runtime packages.
