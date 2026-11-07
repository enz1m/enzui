use std::sync::OnceLock;

pub(super) const FACTORY: &str = "enzuicurlhttpsrc";
pub(super) const RECEIVE_LIMIT: u64 = 256 * 1024;

pub(super) fn register() -> Result<(), &'static str> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    if *REGISTERED.get_or_init(|| {
        curl::init();
        ffi::register_native()
    }) {
        Ok(())
    } else {
        Err("the bounded HTTPS media source could not be registered")
    }
}

#[cfg(test)]
mod tests;

#[allow(unsafe_code, reason = "registers a C GStreamer element")]
mod ffi {
    unsafe extern "C" {
        fn enzui_register_bounded_curl_source() -> i32;
    }

    pub(super) fn register_native() -> bool {
        // SAFETY: the caller has initialized GStreamer, and registration
        // retains only static plugin metadata.
        unsafe { enzui_register_bounded_curl_source() != 0 }
    }
}
