use super::*;

/// A fresh, empty data root under the system temp directory.
fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("enzui-app-icon-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp root");
    dir
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    std::fs::write(path, b"icon").expect("write");
}

#[test]
fn absolute_paths_are_used_as_given_and_relative_paths_are_refused() {
    let data = root("paths");
    let file = data.join("custom.svg");
    touch(&file);
    let roots = [data.clone()];
    let absolute = file.to_str().expect("utf-8");
    assert_eq!(find_app_icon(absolute, 16, &roots), Some(file.clone()));
    assert_eq!(
        find_app_icon(&format!("{absolute}.missing"), 16, &roots),
        None
    );
    touch(&data.join("icons/hicolor/scalable/apps/../../../escape.svg"));
    assert_eq!(find_app_icon("../../../escape", 16, &roots), None);
    assert_eq!(find_app_icon("  ", 16, &roots), None);
    assert_eq!(find_app_icon("missing", 16, &roots), None);
    let _ = std::fs::remove_dir_all(&data);
}
