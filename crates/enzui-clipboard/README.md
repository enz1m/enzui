# enzui-clipboard

The Wayland clipboard on the application's **own** connection and seat. It
uses the ordinary `wl_data_device` protocol, as a focused client, rather than
opening a second display connection or using a data-control extension.

## What to reach for first

- `WaylandClipboard::adopt`: attach to an existing `wayland_client::Connection`.
- `WaylandClipboard::offer(Payload::..)` and `WaylandClipboard::paste(Kind::..)`:
  text, HTML, images and file lists, with MIME negotiation.
- `WaylandClipboard::offer_primary` and `paste_primary`: the primary selection
  (middle-click paste) through `zwp_primary_selection_v1`, where the compositor
  offers it.
- `Payload::sensitive_text(SecretText, lifetime)`: protected text with a positive
  lifetime of at most 120 seconds, a password-manager history hint and no exit
  keeper. Only the original source is revoked. Prefer the nonblocking
  `enzui_clipboard_runtime::write_sensitive_text_async` in an iced application.
- `TransferLimits`: byte and blocked-I/O bounds for every transfer.
- `drag`: native drag and drop with other applications on the same
  connection, with an optional image under the pointer (`DragIcon`) and a
  window carried along with the drag (`attach_window`); see
  [`docs/native-drag-and-drop.md`](../../docs/native-drag-and-drop.md).
- `keeper::hand_off`: when the app exits, a keeper process (the app's own
  executable, diverted before `main`) keeps the last copy pasteable through
  `ext_data_control_v1`, where the compositor offers it. `app_setup::run`
  calls it.

```rust,no_run
use enzui_clipboard::{Kind, Payload, TransferLimits, WaylandClipboard};
use enzui_clipboard::bootstrap::AdoptionSeal;

# fn example(connection: &wayland_client::Connection, seal: &AdoptionSeal) -> Result<(), Box<dyn std::error::Error>> {
let clipboard = WaylandClipboard::adopt(connection, TransferLimits::default(), seal)?;
clipboard.offer(Payload::html("<b>bold</b>", "bold"))?;
let pasted = clipboard.paste(Kind::Html)?;
# Ok(())
# }
```

## Rules the protocol imposes

- An offer needs keyboard focus and a recent input serial, and is only real once
  the compositor confirms the selection.
- The clipboard owner must stay alive while it owns the selection, because it
  serves every paste.
- Transfers can outlive backend teardown. A `DisplayLease` keeps the display
  owner alive for detached workers.

## Inside an iced app

Most apps do not use this crate directly: `enzui-clipboard-runtime`
installs it into the iced runtime.

## Testing

The `test-seal` feature builds `enzui-clipboard-peer`, a line-driven clipboard
peer that the integration tests run as an independent process.
