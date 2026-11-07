//! Compiles the vendored GStreamer curl source, the bounded media source,
//! when the `gstreamer` feature is on.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_assert_message,
    reason = "a build script fails the build by panicking"
)]

fn main() {
    println!("cargo:rerun-if-changed=../../vendor/gst-curl");
    if std::env::var_os("CARGO_FEATURE_GSTREAMER").is_none() {
        return;
    }

    let gst = pkg_config::Config::new()
        .atleast_version("1.14")
        .probe("gstreamer-base-1.0")
        .expect("the bounded media source requires GStreamer 1.14 development headers");
    let curl = pkg_config::Config::new()
        .atleast_version("7.68")
        .probe("libcurl")
        .expect("the bounded media source requires libcurl development headers");
    let mut build = cc::Build::new();
    build
        .files([
            "../../vendor/gst-curl/gstcurlhttpsrc.c",
            "../../vendor/gst-curl/gstcurlqueue.c",
        ])
        .includes(gst.include_paths)
        .includes(curl.include_paths)
        .define("_FILE_OFFSET_BITS", "64")
        .define("GETTEXT_PACKAGE", "\"enzui\"")
        .compile("enzui_bounded_curl");
}
