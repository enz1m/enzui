# Bundled native source

Only `gst-curl/` is bundled in this repository. It contains the bounded Curl
HTTP source compiled by the optional `enzui-services/gstreamer` feature.
Its README, license and `UPSTREAM.json` record the source and alterations.

The Rust foundation forks are public, revision-pinned Git dependencies.
See [Dependencies](../docs/dependencies.md) for their URLs, pins and consumer
workspace overrides. No machine-local iced or winit checkout is required.
