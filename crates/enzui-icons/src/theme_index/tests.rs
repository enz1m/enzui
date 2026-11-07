use super::*;

/// A fresh, empty icon root under the system temp directory.
fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("enzui-theme-index-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp root");
    dir
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    std::fs::write(path, b"<svg/>").expect("write");
}

fn write_theme(root: &Path, name: &str, index: &str) {
    std::fs::create_dir_all(root.join(name)).expect("theme dir");
    std::fs::write(root.join(name).join("index.theme"), index).expect("index");
}

const SCALABLE: &str = "\
[Icon Theme]
Name=Test
Inherits=Parent
Directories=scalable/mimetypes,scalable/places

[scalable/mimetypes]
Size=128
MinSize=8
MaxSize=512
Type=Scalable

[scalable/places]
Size=128
MinSize=8
MaxSize=512
Type=Scalable
";

const PARENT: &str = "\
[Icon Theme]
Name=Parent
Directories=16x16/mimetypes,48x48/mimetypes

[16x16/mimetypes]
Size=16
Type=Fixed

[48x48/mimetypes]
Size=48
Type=Fixed
";

#[test]
fn icon_theme_resolution_obeys_precedence_size_and_path_admission_1() {
    // the_selected_themes_generic_icon_beats_an_inherited_exact_one
    {
        let data = root("order");
        write_theme(&data, "test", SCALABLE);
        write_theme(&data, "Parent", PARENT);
        touch(&data.join("test/scalable/mimetypes/image-x-generic.svg"));
        touch(&data.join("Parent/48x48/mimetypes/image-png.png"));
        let index = ThemeIndex::load("test", std::slice::from_ref(&data));
        assert_eq!(index.search_order(), ["test", "Parent"]);
        assert!(index.has_selected());
        assert_eq!(
            index.find(&["image-png", "image-x-generic"], 24, 1),
            Some(data.join("test/scalable/mimetypes/image-x-generic.svg"))
        );
        // A name only the parent has is still found there.
        assert_eq!(
            index.find(&["image-png"], 48, 1),
            Some(data.join("Parent/48x48/mimetypes/image-png.png"))
        );
        assert_eq!(index.find(&["missing"], 24, 1), None);
        let _ = std::fs::remove_dir_all(&data);
    }

    // fixed_directories_prefer_the_exact_size_then_the_closest
    {
        let data = root("sizes");
        write_theme(&data, "Parent", PARENT);
        touch(&data.join("Parent/16x16/mimetypes/text-plain.png"));
        touch(&data.join("Parent/48x48/mimetypes/text-plain.png"));
        let index = ThemeIndex::load("Parent", std::slice::from_ref(&data));
        assert_eq!(
            index.find(&["text-plain"], 16, 1),
            Some(data.join("Parent/16x16/mimetypes/text-plain.png"))
        );
        assert_eq!(
            index.find(&["text-plain"], 40, 1),
            Some(data.join("Parent/48x48/mimetypes/text-plain.png"))
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    // an_earlier_root_wins_and_a_theme_may_span_roots
    {
        let (user, system) = (root("user"), root("system"));
        write_theme(&system, "test", SCALABLE);
        touch(&user.join("test/scalable/places/folder.svg"));
        touch(&system.join("test/scalable/places/folder.svg"));
        touch(&system.join("test/scalable/places/user-home.svg"));
        let roots = [user.clone(), system.clone()];
        let index = ThemeIndex::load("test", &roots);
        assert_eq!(
            index.find(&["folder"], 24, 1),
            Some(user.join("test/scalable/places/folder.svg"))
        );
        assert_eq!(
            index.find(&["user-home"], 24, 1),
            Some(system.join("test/scalable/places/user-home.svg"))
        );
        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&system);
    }

    // a_missing_theme_searches_only_hicolor_and_bad_names_are_refused
    {
        let data = root("missing");
        let index = ThemeIndex::load("nope", std::slice::from_ref(&data));
        assert!(!index.has_selected());
        assert_eq!(index.find(&["folder"], 24, 1), None);
        write_theme(&data, "test", SCALABLE);
        touch(&data.join("test/scalable/places/folder.svg"));
        let index = ThemeIndex::load("test", std::slice::from_ref(&data));
        assert_eq!(index.find(&["../places/folder", "a/b", ""], 24, 1), None);
        let _ = std::fs::remove_dir_all(&data);
    }
}

#[test]
fn icon_theme_resolution_obeys_precedence_size_and_path_admission_2() {
    // index_parsing_skips_escaping_and_malformed_directories
    {
        let theme = parse_index_theme(
            "t",
            "[Icon Theme]\nInherits=A, B,bad/name\nDirectories=ok,../escape,nosection,badsize\n\
         [ok]\nSize=32\nType=Threshold\n[../escape]\nSize=16\n[badsize]\nSize=x\n",
        )
        .expect("theme");
        assert_eq!(theme.inherits, ["A", "B"]);
        assert_eq!(theme.directories.len(), 1);
        assert_eq!(theme.directories[0].dir, "ok");
        assert!(parse_index_theme("t", "[Other]\nDirectories=a\n").is_none());
    }

    // roots_put_home_icons_before_xdg_data_dirs
    {
        let env = Globs2Env::snapshot(|key| match key {
            "HOME" => Some("/home/u".into()),
            "XDG_DATA_DIRS" => Some("/usr/local/share:/usr/share".into()),
            _ => None,
        });
        assert_eq!(
            icon_theme_roots_from(&env),
            [
                PathBuf::from("/home/u/.icons"),
                PathBuf::from("/home/u/.local/share/icons"),
                PathBuf::from("/usr/local/share/icons"),
                PathBuf::from("/usr/share/icons"),
            ]
        );
    }
}
