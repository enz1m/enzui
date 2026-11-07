# Native Wayland drag and drop

`enzui_clipboard_runtime::native_drag` exchanges MIME data and local file URI
lists with other applications through the ordinary `wl_data_device` protocol.
Normal `enzui_widgets::app_setup::run` and `run_with_traversal` install the
transport with the Wayland clipboard. It reuses the application's display,
seat, data device and bounded transfer workers. It does not use a privileged
clipboard-manager service or open a second display connection.

## Text drag and drop

`app_setup::run` connects this runtime itself for text drag and drop
(`enzui_widgets::text_drag`): selected text drags out of fields and editors,
and dropped text goes into them, with nothing to wire. Because one runtime
owns destination events, an application that connects `NativeDragRuntime`
for its own drops (files, images) turns the service off with
`AppServices::default().text_drag(false)` and handles text drops itself.

Read-only selections in `enz_selection_scope` and `enz_glyph_selection` drag
out as Copy, preserving the source document. Press inside the visible selected
text and move past the desktop drag threshold; an ordinary click retains the
widget's normal selection behavior. Replacing or closing the document retires
a pending gesture.

Custom canvases use `text_drag::offer_read_only_selection` from `Widget::operate`.
Its resolver runs only for a source press, with a point normalized through scroll
translations. Offer text only for visible selected geometry, and return a
cancellation token that is retired with the document/selection owner. The shared
host handles the native Copy gesture, popup routing and ordinary-click fallback;
text is limited to 8 MiB. This also supports terminal transcript selections.

## Dropping files

Wrap the part of the view that takes files in
`file_drop::enz_file_drop_target(content, on_drop)`: files dragged onto it
from a file manager or another application arrive as one message with their
paths. `app_setup::run` carries them over the same native connection as text
drops, so nothing else is wired and text fields keep taking dropped text.
Negotiation follows the innermost compatible target under the pointer,
including scroll offsets and clipped viewports. Only that region receives
hover feedback. MIME-incompatible children allow a compatible parent to take
the drop; an area with no matching target refuses it. Read-only text fields
refuse native text drops.

That target tells the source the drop succeeded as soon as it takes the
paths. An app that copies or checks the files first (an attachment staged
into private storage, a batch that must all succeed) uses
`file_drop::enz_file_drop_target_with_receipt(content, on_drop)` instead. Its
message carries a `FileDrop`: `paths()` are the files, and `finish()` tells
the source once they are safely the app's. Dropping the `FileDrop` unfinished
(staging failed or was cancelled, the window closed, or the app refuses drops
while busy) refuses the drop. A temporary export may be removed when its native
source completes or is cancelled; receivers must finish their copy before acknowledging.

A drop belongs to the target under the pointer when the user released it,
although the files arrive a moment later. If that target is gone by then, the
drop is refused rather than handed to whatever replaced it. A target that
stays in place while what it stands for changes (the page or document its
`on_drop` captures) passes that with `.key(page)`: a drop released under one
key is refused unfinished once the target carries another, so the callback
never pairs an earlier drop with a later state. Leaving and returning to a key
also retires the old drop, including data waiting for its callback. Use
`.enabled(can_accept)` to pause a target while preserving its content's focus
and widget state. Disabling retires released and pending drops; child targets
remain independent.

## File sources and custom data

Wrap a local row or thumbnail with
`file_drag::enz_file_drag_source(content, selected_paths)`. It starts after the
desktop drag threshold while the initiating pointer or touch is still held.
Ordinary clicks remain clicks; native handoff cancels the child's pending click.
Use `.allows_captured_press(true)` around a clickable row, keeping it disabled
around selectable text or a competing drag gesture. Selections are bounded to
128 paths; larger selections are refused as a whole.

For attachments held in memory or in private storage,
`enz_file_drag_source_with(content, prepare)` runs preparation on a worker only
after the drag threshold. Give prepared sources a stable `.key(asset_id)` so
replacing a row during a pending gesture cannot export its replacement. Ordinary
sources use their path selection as their default key. Return
`PreparedFiles::owned(paths, owner)`, retaining
for example a temporary directory until destination completion. Check the
provided cancellation token between bounded I/O operations. Preparation does
not run while building or hovering the view; it must finish while the user's
gesture is still held, otherwise native grab admission refuses the start.
Prepared sources are Copy only and advertise an enzUI temporary-files hint.
A receiver can inspect `FileDrop::is_temporary()` and use
`enzui_core::drop_files::copy_files` on its bounded blocking worker before
calling `finish()`. Keep the returned `CopiedFiles` owner while a viewer uses
session copies. For an editable document, copy into a private application import
directory and call `preserve()` after successful load so later saves and recent
files remain usable. `.temporary_files(true)` marks an existing session-owned
path source. The hint is optional metadata, not permission or validation: other
toolkits may omit it, and receivers needing durable imports should always copy.

Failed preparation or an unmounted source
retires unfinished temporary work.

For app-specific rearrangement, a file source may add `.with_data(mime, bytes)`
while retaining its native URI and file-portal formats. Inner targets can prefer
the app format; other applications still receive ordinary files. Key both ends
to their resource/session and validate the identifier at dispatch.
`enz_data_drag_source(content, owner_key, payload)` exports ordinary custom MIME
payloads without file paths. Buffers are shared across clones. Extra data is
limited to 16 representations and 32 MiB in total; sensitive and deferred text
are refused, and metadata cannot replace a file source's URI/portal formats.

Topmost popover targets receive drops in window coordinates, including when the
underlying view is smaller than the window. An opaque popup without a compatible
target blocks drops to the document behind it. Closing or replacing a target
before delivery refuses the drop. Text fields in popovers use the same routing.

The default source offers Copy. `.on_finished(callback)` reports cancellation
or destination completion. A file manager may enable
`.actions(file_drag::DragActions::BOTH)` and perform Move at the destination;
the framework never deletes source paths. Retain application ownership and
refresh source listings after completion.

Import and attachment targets use `.copy_only(true)`: only Copy is negotiated
while that target is under the pointer, regardless of other mounted targets.
File-manager folder targets can use `.preferred_action(DragAction::Move)`;
Copy remains available if Move is absent from the source. `.highlight(false)`
opts out of the theme-based drop feedback when the application supplies its own.
The default feedback tints content with `palette.accent` at
`opacity.drop_target` (0.08 by default, bounded to 0..0.2); its outline uses
`palette.focus_ring` and the row shape. Destination labels remain visible.

`file_drop::enz_data_drop_target(content, mime_preferences, callback)` uses the
same routing, generation keys and feedback for text, encoded images or other
MIME data. Its `DataDrop::data()` retains the actual negotiated `Pasted` value;
call `finish()` after successful application processing. Defaults are Copy only.
This is useful for custom canvases such as terminal panes, which do not expose
an ordinary text input to automatic text-drop routing.

With `portal-dbus`, file targets also resolve
`application/vnd.portal.filetransfer` keys through the document portal. File
Copy sources export read-only regular-file descriptors when the portal is
available; directory and Move sources retain ordinary URI interoperability.
Portal keys are never logged, exports live through destination completion, and
failed or unavailable portal requests do not silently become arbitrary paths.
The low-level helper is `enzui_core::portal::file_transfer`.

## Runtime integration

The rest of this page describes manual runtime integration for hosts that own
the event loop or need lower-level protocol control. The widget wrappers above
already handle ordinary file, custom-MIME and text sources and targets.

After a window opens, run `NativeDragRuntime::connect(budget)` in an iced task
and retain the returned runtime. Include `runtime.subscription()` in the
application's subscription batch and route its `DragEvent` messages to the
reducer. One runtime owns destination events; clones share that consumer rather
than broadcasting duplicate events. A second connection attempt returns `Busy`
while the original consumer remains alive.

Use `iced::window::run(window_id, |window| native_drag::surface_id(window))`
to associate each live Wayland surface with an application window. Remove the
mapping on close because Wayland object ids may be reused. `Entered` supplies
that surface id, seat name, offered MIME types/actions and surface-local logical
coordinates. The application owns drop-region hit testing and feedback; these
events are not synthesized iced `FileDropped` events.

On `Entered`, choose MIME preferences and call `accept(id, preferences, actions,
preferred, budget)`. Acceptance requires a common copy/move operation and a
matching MIME type; no arbitrary fallback is chosen. Track `Motion` and `Action`
for feedback. Call `reject` to permanently retire an unwanted offer. For a temporary
non-target region, accept with `DragActions::NONE` instead, allowing the pointer
to re-enter a valid target within the same surface. `Left` invalidates the offer;
`Reset` clears all destination feedback after event pressure or shutdown.

On `Dropped`, call `receive(id, budget)` once. Inspect `ReceivedDrop::data()` and
`action()`, process the operation, and call `finish()` only after successful
commit. Dropping the received value without finishing cancels it. A move source
must not delete anything merely because bytes were read or the pointer was
released. Reaccepting a dropped offer, receiving twice, and requests using a
retired identity fail.

## Starting a native gesture

Call `runtime.start(payload, actions, seat, budget)` while the originating
pointer button or touch is still held. Keyboard focus, keyboard serials and
past pointer activity do not authorize a drag. The dispatch worker uses the
current implicit grab's serial and origin surface. Start at the gesture's drag
threshold rather than after a button's release callback. Without an icon the
compositor shows only the cursor; `runtime.start_with_icon(payload, actions,
seat, Some(icon), budget)` puts an image under the pointer for the drag.
`DragIcon::from_rgba(rgba, width, height, hotspot, scale)` makes one from
straight RGBA pixels (at most 1024 pixels a side); a hotspot outside the image,
such as `(-12, -12)`, keeps it beside the pointer. Text drags show a snippet of
the dragged text.

### Dragging a window along

A tab torn out of a tab bar can travel as its own window. Once the drag has
started, open the new window, resolve its toplevel with
`iced::window::wayland_toplevel(id)`, and pass that to
`runtime.attach_window(toplevel, offset, budget)`, where `offset` is the point
of the window the pointer holds, in its logical pixels. The window follows the
pointer and stays where the drag ends (`xdg_toplevel_drag_v1`). Where the
compositor does not offer the protocol the call is refused; place the window
yourself on drop.

Keep the returned `SourceDrag` alive, normally by passing it to
`native_drag::source_outcome` in an iced task. `Finished(Copy)` or
`Finished(Move)` means the destination sent protocol completion. Only the latter
permits removal of the source data. Dropping the source or its pending outcome
future, or calling `cancel`, requests cancellation. Compositor cancellation and
seat/connection teardown also terminate the source.

A successful start request is not compositor acknowledgement. If neither a
pointer leave nor a source event confirms activity within two seconds, the
source is cancelled. This bounds a serial that became invalid before the
compositor handled it. An unconfirmed touch drag outside all destinations can
also hit that deadline. Confirmed drags have no arbitrary gesture-length limit;
the application can cancel a peer that never completes.

A release of the initiating pointer button delivered outside the native drag
cancels an outstanding start attempt. Subsequent clicks after the native drop
leave destination processing active until completion or explicit cancellation.

## Files, limits and compatibility

For files, offer `Payload::uris(paths)` and negotiate `Kind::Uris.preferences()`.
Use the received `Pasted::file_transfer()` to validate and decode local file
URIs. The compositor-selected `DragAction`, not a clipboard copy/cut header,
determines native copy/move semantics. URI transfer does not itself copy, move,
open or delete filesystem entries.

Native payloads permit up to 64 MIME names of at most 1024 bytes each. Empty
names and control characters are refused. Each representation obeys the
configured transfer byte ceiling. Incoming native offers exceeding the MIME
limits are rejected; ordinary clipboard metadata retains its existing behavior.
Native command admission and destination event storage each have 64 slots;
adjacent motion events coalesce. Event overflow resets and cancels destination
offers. At most 16 incoming offers can be retained across hover, receive and
commit; reads/writes share clipboard transfer permits and timeouts. Drop unused
runtime/received values to release retained work.

Async methods return `None` for unavailable registration, admission failure or
budget expiry, and preserve typed transport errors in `Some(Err(..))`. A timeout
does not undo a request already started. Native sources require data-device
manager version 3; older destinations can receive copy-only offers.

X11 retains its existing clipboard/window behavior. Native Wayland access is
unavailable on X11 or when `ENZUI_CLIPBOARD=default` explicitly retains iced's
provider. Internal `enzui_core::drag`, `EnzDragDropHost` and reorder widgets remain
local APIs; they do not automatically export a native source. Compositor pointer
leave cancels their local pointer session. Choose one gesture owner rather than
starting local and native operations independently for the same press.

## Verification boundary

The `app_setup_clipboard` target's `native_drag` cases run real iced application
reducers and an independent GTK 3 peer on private headless Sway, with private
D-Bus and XDG directories. They exercise text copy and file-URI move in both
directions, public file-source/target widgets through the default services,
rejection, cancellation and destination commit. The file-transfer portal tests
use a private D-Bus peer to verify real descriptor bytes, bounded path admission
and read-only export lifetime. The
`wayland_data_device` ignored-start case uses a synthetic protocol server to
force silent refusal. Neither fixture constitutes validation of a user's live
desktop or every compositor implementation.
