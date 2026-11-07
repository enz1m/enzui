# Install a WPE web engine consumer

The toolkit-free engine lives in the sibling `enzui-webkit` repository. Its
`ffi::deployment::set_credential_guard_directory` selects a trusted installed
directory before WPE initialization; package the matching native guard with the
application. The directory must be absolute and contain the readable regular
`libenzui_credential_guard.so` built from the same engine source. The selection
is process-wide and freezes after the first native initialization attempt,
including a failed attempt. Keep the directory trusted and available for the
process lifetime. Handle directory-selection errors before creating a display.
Sandbox registration and the guard ready handshake remain required; native
session construction reports `CredentialGuardUnavailable` when it cannot
establish the guard.

The engine crate's `enzui-webkit/crates/enzui-webkit/README.md`, under
*Install the mandatory guard*, covers build artifact discovery, runtime setup
and error handling. `ffi::deployment::credential_guard_build_directory` locates
the build artifact for packaging; installed applications must select their own
installed directory before creating a WPE display. GUI applications use the
sibling `enzui-webview` adapter; the framework itself does not embed the engine.
