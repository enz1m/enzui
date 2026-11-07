//! Event sound lookup over a made-up set of themes: user directory first,
//! inheritance, shorter names, `.disabled`, stereo first and unthemed sounds.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking"
)]
use enzui_core::sound_theme::find_event_sound;
use std::path::{Path, PathBuf};

fn put(path: PathBuf, contents: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, contents).unwrap();
    path
}

/// A user and a system `sounds` directory.
fn themes(name: &str) -> (PathBuf, Vec<PathBuf>) {
    let root = std::env::temp_dir().join(format!("enzui-sounds-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (user, system) = (root.join("user/sounds"), root.join("system/sounds"));
    let index = |inherits: &str| {
        format!(
            "[Sound Theme]\nName=x\nInherits={inherits}\nDirectories=stereo\n\n[stereo]\nOutputProfile=stereo\n"
        )
    };
    put(system.join("ocean/index.theme"), &index("base"));
    put(system.join("base/index.theme"), &index(""));
    put(system.join("freedesktop/index.theme"), &index(""));
    put(
        system.join("freedesktop/stereo/message-new-instant.oga"),
        "",
    );
    put(system.join("freedesktop/stereo/bell.oga"), "");
    put(system.join("freedesktop/stereo/complete.wav"), "");
    put(system.join("base/stereo/message.oga"), "");
    put(system.join("ocean/stereo/message-new-instant.oga"), "");
    put(system.join("ocean/stereo/complete.disabled"), "");
    put(user.join("ocean/stereo/message-new-instant.ogg"), "");
    put(system.join("custom-ding.wav"), "");
    (root, vec![user, system])
}

fn relative(root: &Path, found: Option<PathBuf>) -> Option<String> {
    found.map(|path| path.strip_prefix(root).unwrap().display().to_string())
}

#[test]
fn sound_theme_resolves_inheritance_and_terminates_cycles() {
    // a_sound_comes_from_the_theme_its_parents_or_freedesktop
    {
        let (root, dirs) = themes("lookup");
        let find = |name, theme| relative(&root, find_event_sound(name, theme, &dirs));
        assert_eq!(
            find("message-new-instant", "ocean").as_deref(),
            Some("user/sounds/ocean/stereo/message-new-instant.ogg"),
            "the user's copy of the theme first"
        );
        assert_eq!(
            find("bell", "ocean").as_deref(),
            Some("system/sounds/freedesktop/stereo/bell.oga"),
            "freedesktop has what the theme lacks"
        );
        assert_eq!(
            find("message-new-email", "ocean").as_deref(),
            Some("system/sounds/base/stereo/message.oga"),
            "a shorter name from an inherited theme"
        );
        assert_eq!(find("complete", "ocean"), None, "the theme switched it off");
        assert_eq!(
            find("complete", "base").as_deref(),
            Some("system/sounds/freedesktop/stereo/complete.wav")
        );
        assert_eq!(
            find("custom-ding", "ocean").as_deref(),
            Some("system/sounds/custom-ding.wav"),
            "an unthemed sound"
        );
        assert_eq!(find("no-such-sound", "ocean"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    // inheritance_loops_end_and_stereo_comes_first
    {
        let (root, dirs) = themes("loops");
        let system = &dirs[1];
        put(
            system.join("a/index.theme"),
            "[Sound Theme]\nInherits=b\nDirectories=5.1,stereo\n\n[5.1]\nOutputProfile=5.1\n\n[stereo]\nOutputProfile=stereo\n",
        );
        put(
            system.join("b/index.theme"),
            "[Sound Theme]\nInherits=a\nDirectories=stereo\n",
        );
        put(system.join("a/5.1/alarm.oga"), "");
        put(system.join("a/stereo/alarm.oga"), "");
        assert_eq!(
            relative(&root, find_event_sound("alarm", "a", &dirs)).as_deref(),
            Some("system/sounds/a/stereo/alarm.oga")
        );
        assert_eq!(
            relative(&root, find_event_sound("bell", "a", &dirs)).as_deref(),
            Some("system/sounds/freedesktop/stereo/bell.oga")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
