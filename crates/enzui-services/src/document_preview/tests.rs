use super::*;
use crate::decode_limits::DecodeLimits;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[cfg(target_os = "linux")]
fn preview_stage_dir(label: &str) -> PathBuf {
    let root = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!(
            "preview-stage-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[cfg(target_os = "linux")]
#[test]
fn stage_hook_before_copy_append_is_refused() {
    use std::io::Write;

    let root = preview_stage_dir("pre-append");
    let source = root.join("source.bin");
    let admitted: &[u8] = b"admitted-prefix-64-bytes-.......................................";
    std::fs::write(&source, admitted).unwrap();

    let path = source.clone();
    let _guard = super::stage_hook::Guard;
    super::stage_hook::install(move |point| {
        if point == super::stage_hook::Point::BeforeCopy {
            std::thread::sleep(Duration::from_millis(20));
            let mut appender = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            appender.write_all(&vec![0xFEu8; 4096]).unwrap();
            appender.flush().unwrap();
        }
    });

    let Err(err) = open_checked_file(&source, &DecodeLimits::DEFAULT) else {
        panic!("a pre-copy append must not produce a snapshot")
    };
    assert!(
        matches!(err, PreviewError::Io { .. }),
        "expected PreviewError::Io, got {err:?}",
    );

    std::fs::remove_dir_all(root).unwrap();
}
