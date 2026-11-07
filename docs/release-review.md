# Alpha 0.1 validation

This alpha is distributed from the public GitHub repository with one signed
initial commit and the signed `v0.1.0-alpha.1` tag. The framework has no crates.io
release; `publish = false` remains until registry packaging is qualified.

Rust foundations are public, revision-pinned dependencies. The bounded native
media source and its provenance remain under `vendor/gst-curl`. See
[dependencies](dependencies.md), [platform requirements](platform-support.md)
and [third-party notices](../THIRD_PARTY.md).

## Build and documentation checks

The preview was checked on Linux x86_64 with Rust 1.99.1. The separate Rust
1.95 check covers all workspace targets and features. These commands reproduce
the development gates after installing the native prerequisites:

```sh
cargo fmt --all -- --check
cargo wcheck --locked
cargo wclippy
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --all-features --no-deps
cargo +1.95.0 wcheck --locked
```

All-feature checks include internal observation and fault-injection features;
they are not a production application configuration. The checked-in profiles
and cache/optimization choices are described in [build performance](build-performance.md).
A standalone consumer must carry the [documented overrides](dependencies.md#consumer-workspace-overrides);
Cargo does not inherit dependency patch tables.

The public Markdown pages were reviewed against the current feature manifests,
API sources and runnable examples. Generated widget, token and capability
indexes are checked against their sources. Relative file links and Markdown
section links are checked separately from rustdoc's API links.

The complete App and Gallery examples demonstrate the framework's own window
frame. Catalog snippets are compile-only fixtures; they are not additional
runnable widget demos. See [building an app](building-an-app.md).

## Behavior and native integration

Focused verification covers input/focus routing, theme roles and sizing,
resource ownership, descriptor handling, persistence boundaries and generated
contracts. The complete retained test suite has not been executed in this
review. A source test count alone does not describe which platform/feature
paths were exercised.

Native drag-and-drop cases ran with real iced reducers and an independent
GTK peer on a private headless Wayland desktop. They cover files, text,
ordinary custom MIME data, delayed completion, refusal, cancellation and
resource retirement. Temporary-file custody and receiver-owned imports were
also checked. The latest drag-source regression exercises the first held movement
leaving a scrollable viewport: disabling the fix makes the GTK receiver time out,
and restoring it completes the transfer. The native target passed 15 workflows;
57 widget workflows, formatting and strict widget Clippy also passed. Light and
dark drop feedback uses a tint without an outline. Local app checks cover HEIC,
documents, media, attachments and text; a video player was observed opening the
original local path rather than making a playback copy.

The GNOME Documents file-transfer portal was exercised between isolated app
profiles on the desktop session bus. Portal tests also use a private D-Bus peer
to exercise real descriptor bytes and lifetime bounds. See [the native verification boundary](native-drag-and-drop.md#verification-boundary).

A Wayland-only consumer and an additive X11 build were checked separately.
X11 window mapping does not qualify Wayland-only capabilities on X11.
Headless GPU replay comparisons check rendered frames; native compositor
presentation and visible latency need their own capture evidence.

## Remaining qualification

This preview does not qualify every compositor, touch/seat combination,
Flatpak portal backend, live media service or accessibility client. Toolkit
attachment exports that omit lifetime metadata need an application ownership
policy; a URI alone does not promise a permanent file.

Non-Linux platforms are unsupported. The sample Flatpak manifest is a local
packaging starting point, not a qualified installed package. Hosted validation status is reported by the repository's
[Rust checks workflow](../.github/workflows/ci.yml). docs.rs execution has not
been qualified.
Source/credential-pattern scans and targeted boundary tests do not constitute
a security audit.
