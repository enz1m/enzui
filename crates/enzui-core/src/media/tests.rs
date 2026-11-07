use super::*;

#[test]
fn remote_source_refuses_file_scheme() {
    let policy = UrlPolicy::DEFAULT;
    let result = MediaSource::remote("file:///etc/passwd", &policy);
    assert!(
        result.is_err(),
        "file:// must be refused as a remote source"
    );
}
