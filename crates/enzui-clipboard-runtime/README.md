# enzui-clipboard-runtime

Installs the `enzui-clipboard` Wayland clipboard into an iced
application's runtime, on the event loop's existing Wayland connection.

## Usage

`enzui_widgets::app_setup::run` installs the provider before starting the event
loop. Hosts that own their event loop call `adopt_process_clipboard()`, or
`adopt_process_clipboard_with(..)` to choose the policy in code. The default
uses the ordinary Wayland clipboard; setting the environment variable named by
`enzui_clipboard::bootstrap::POLICY_VARIABLE` to `default` keeps iced's own
clipboard instead. An explicit `adopt_process_clipboard_with(..)` call before
application startup fixes the process choice, so the later automatic setup does
not override it.

An *adopted* outcome means the provider was installed. It does not mean a
display was connected or that any transfer succeeded.

- The provider connects once per event loop and opens no extra Wayland client.
- Non-Wayland displays keep the runtime's default clipboard.
- Transfers can outlive backend teardown: the worker transport and dispatch
  thread hold the event loop's display lease until they finish with it.

Beyond what iced's clipboard API carries, `read_file_transfer` and
`write_file_transfer` (and their `_async` forms) move file lists with a
copy/cut intent, and `holds_keyboard_focus` reports whether a seat has focus,
which an offer needs.

## Sensitive text

Use `write_sensitive_text_async(SecretText, lifetime, response_budget)` from a
focused user activation for passwords or one-time codes. It uses the installed
Wayland bridge, advertises `x-kde-passwordManagerHint=secret`, skips the exit
keeper, and revokes the original source at expiry. It never clears a newer
source. The hint asks peers to skip history; it cannot erase text a reader has
already received. A compositor clipboard manager can retain its own copy after
source revocation, including on GNOME; expiry is not a guarantee that a later
paste from such a manager is empty. Protected storage is wiped when its last holder releases it;
revocation interrupts blocked sends within a 50 ms poll interval.

Lifetimes must be positive and at most 120 seconds; text is limited to 64 KiB.
`Some(Ok(()))` confirms ownership, `Some(Err(error))` reports refusal, and `None`
means unavailable, rejected or incomplete. Do not report `None` as success or
fall back to ordinary `Content::Text`.

## Features

| feature | default | what |
|---|---|---|
| `wayland` | yes | Wayland window and clipboard backend |
| `x11` | no | optional X11 backend; other clipboard behavior follows iced |
| `image` | no | PNG clipboard images, converted to and from iced's RGBA |

Text, HTML and local file paths work without `image`. PNG dimensions and size
are checked against `enzui_core::decode_limits` before any pixels are
allocated.
