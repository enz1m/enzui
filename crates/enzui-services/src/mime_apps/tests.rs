#![allow(
    clippy::cast_possible_truncation,
    reason = "test fixtures write known small lengths"
)]

use super::*;

struct Scratch(PathBuf);

impl Scratch {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Profile {
    root: Scratch,
}

impl Profile {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("enzui-mime-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self {
            root: Scratch(path),
        }
    }

    fn context(&self, desktops: &[&str]) -> MimeContext {
        MimeContext {
            config_home: self.root.path().join("config"),
            config_dirs: vec![self.root.path().join("etc-xdg")],
            data_home: self.root.path().join("data"),
            data_dirs: vec![self.root.path().join("usr-share")],
            desktops: desktops.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.root.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn app(&self, root: &str, id: &str, name: &str, mime: &str) {
        self.write(
            &format!("{root}/applications/{id}"),
            &format!(
                "[Desktop Entry]\nType=Application\nName={name}\nIcon={name}-icon\n\
                 Exec=\"/opt/My Apps/{name}\" %U\nMimeType={mime}\n"
            ),
        );
    }
}

#[test]
fn mime_handler_precedence_and_updates_preserve_effective_associations_1() {
    // oversized_files_and_cancelled_scans_are_refused
    {
        let profile = Profile::new();
        profile.app("usr-share", "a.desktop", "A", "text/plain;");
        let apps = MimeApps::new(profile.context(&[]));
        let cancelled = CancelToken::new();
        let _ = cancelled.cancel(enzui_core::cancellation::CancelCause::Cancelled);
        assert!(matches!(
            apps.candidates("text/plain", &cancelled),
            Err(MimeError::Cancelled)
        ));

        profile.write(
            "config/mimeapps.list",
            &"#".repeat(MAX_FILE_BYTES as usize + 1),
        );
        assert!(matches!(
            apps.default_for("text/plain", &CancelToken::new()),
            Err(MimeError::TooLarge(path)) if path.ends_with("mimeapps.list")
        ));
    }

    // defaults_require_an_effective_association
    {
        let profile = Profile::new();
        profile.app("usr-share", "unrelated.desktop", "Unrelated", "image/png;");
        profile.app("usr-share", "valid.desktop", "Valid", "text/plain;");
        profile.write(
            "config/mimeapps.list",
            "[Default Applications]\ntext/plain=unrelated.desktop;valid.desktop;\n",
        );
        let apps = MimeApps::new(profile.context(&[]));
        let default = apps
            .default_for("text/plain", &CancelToken::new())
            .unwrap()
            .unwrap();
        assert_eq!(default.app.id, "valid.desktop");
        assert!(default.explicit);
    }

    // removed_associations_invalidate_a_default
    {
        let profile = Profile::new();
        profile.app("usr-share", "removed.desktop", "Removed", "text/plain;");
        profile.app("usr-share", "valid.desktop", "Valid", "text/plain;");
        profile.write("config/mimeapps.list", "[Default Applications]\ntext/plain=removed.desktop;valid.desktop;\n[Removed Associations]\ntext/plain=removed.desktop;\n");
        let apps = MimeApps::new(profile.context(&[]));
        assert_eq!(candidate_ids(&apps, "text/plain"), ["valid.desktop"]);
        assert_eq!(
            apps.default_for("text/plain", &CancelToken::new())
                .unwrap()
                .unwrap()
                .app
                .id,
            "valid.desktop"
        );
    }

    // a_specific_default_wins_before_a_higher_precedence_parent_default
    {
        let profile = Profile::new();
        profile.app("usr-share", "parent.desktop", "Parent", "text/plain;");
        profile.app("usr-share", "child.desktop", "Child", "text/x-enzui-child;");
        profile.write("data/mime/subclasses", "text/x-enzui-child text/plain\n");
        profile.write(
            "config/mimeapps.list",
            "[Default Applications]\ntext/plain=parent.desktop;\n",
        );
        profile.write(
            "usr-share/applications/mimeapps.list",
            "[Default Applications]\ntext/x-enzui-child=child.desktop;\n",
        );
        let apps = MimeApps::new(profile.context(&[]));
        let default = apps
            .default_for("text/x-enzui-child", &CancelToken::new())
            .unwrap()
            .unwrap();
        assert_eq!(default.app.id, "child.desktop");
        assert!(default.explicit);
    }
}

fn candidate_ids(apps: &MimeApps, mime: &str) -> Vec<String> {
    apps.candidates(mime, &CancelToken::new())
        .unwrap()
        .into_iter()
        .map(|app| app.id)
        .collect()
}

#[test]
fn mime_handler_precedence_and_updates_preserve_effective_associations_2() {
    // a_specific_implicit_handler_wins_before_a_parent_default
    {
        let profile = Profile::new();
        profile.app("usr-share", "parent.desktop", "Parent", "text/plain;");
        profile.app("usr-share", "child.desktop", "Child", "text/x-enzui-child;");
        profile.write("data/mime/subclasses", "text/x-enzui-child text/plain\n");
        profile.write(
            "config/mimeapps.list",
            "[Default Applications]\ntext/plain=parent.desktop;\n",
        );
        let apps = MimeApps::new(profile.context(&[]));
        let default = apps
            .default_for("text/x-enzui-child", &CancelToken::new())
            .unwrap()
            .unwrap();
        assert_eq!(default.app.id, "child.desktop");
        assert!(!default.explicit);
    }

    // an_explicit_child_default_can_use_inherited_support
    {
        let profile = Profile::new();
        profile.app("usr-share", "parent.desktop", "Parent", "text/plain;");
        profile.app("usr-share", "child.desktop", "Child", "text/x-enzui-child;");
        profile.write("data/mime/subclasses", "text/x-enzui-child text/plain\n");
        profile.write(
            "config/mimeapps.list",
            "[Default Applications]\ntext/x-enzui-child=parent.desktop;\n",
        );
        let apps = MimeApps::new(profile.context(&[]));
        let default = apps
            .default_for("text/x-enzui-child", &CancelToken::new())
            .unwrap()
            .unwrap();
        assert_eq!(default.app.id, "parent.desktop");
        assert!(default.explicit);
    }

    // desktop_specific_files_only_supply_defaults
    {
        let profile = Profile::new();
        profile.app("usr-share", "invalid.desktop", "Invalid", "image/png;");
        profile.app("usr-share", "valid.desktop", "Valid", "text/plain;");
        profile.write("config/gnome-mimeapps.list", "[Added Associations]\ntext/plain=invalid.desktop;\n[Removed Associations]\ntext/plain=valid.desktop;\n[Default Applications]\ntext/plain=valid.desktop;\n");
        let apps = MimeApps::new(profile.context(&["gnome"]));
        assert_eq!(candidate_ids(&apps, "text/plain"), ["valid.desktop"]);
        assert!(
            apps.default_for("text/plain", &CancelToken::new())
                .unwrap()
                .unwrap()
                .explicit
        );
    }

    // lower_directory_removals_do_not_override_user_desktop_entries
    {
        let profile = Profile::new();
        profile.app("data", "same.desktop", "User", "text/plain;");
        profile.app("usr-share", "same.desktop", "System", "text/plain;");
        profile.write(
            "usr-share/applications/mimeapps.list",
            "[Removed Associations]\ntext/plain=same.desktop;\n",
        );
        let apps = MimeApps::new(profile.context(&[]));
        assert_eq!(candidate_ids(&apps, "text/plain"), ["same.desktop"]);
        assert_eq!(
            apps.application("same.desktop", &CancelToken::new())
                .unwrap()
                .unwrap()
                .name,
            "User"
        );
    }
}

#[test]
fn mime_handler_precedence_and_updates_preserve_effective_associations_3() {
    // lower_directory_additions_do_not_change_a_user_override
    {
        let profile = Profile::new();
        profile.app("data", "same.desktop", "User", "image/png;");
        profile.app("usr-share", "same.desktop", "System", "text/plain;");
        profile.app("usr-share", "valid.desktop", "Valid", "text/plain;");
        profile.write(
            "usr-share/applications/mimeapps.list",
            "[Added Associations]\ntext/plain=same.desktop;\n",
        );
        let apps = MimeApps::new(profile.context(&[]));
        assert_eq!(candidate_ids(&apps, "text/plain"), ["valid.desktop"]);
    }

    // a_parent_removal_does_not_cancel_a_more_specific_association
    {
        let profile = Profile::new();
        profile.app("usr-share", "chosen.desktop", "Chosen", "image/png;");
        profile.write("data/mime/subclasses", "text/x-enzui-child text/plain\n");
        profile.write(
            "config/mimeapps.list",
            "[Removed Associations]\ntext/plain=chosen.desktop;\n",
        );
        profile.write(
            "usr-share/applications/mimeapps.list",
            "[Added Associations]\ntext/x-enzui-child=chosen.desktop;\n",
        );
        let apps = MimeApps::new(profile.context(&[]));
        assert_eq!(
            candidate_ids(&apps, "text/x-enzui-child"),
            ["chosen.desktop"]
        );
    }

    // a_valid_added_association_can_supply_an_explicit_default
    {
        let profile = Profile::new();
        profile.app("usr-share", "chosen.desktop", "Chosen", "image/png;");
        profile.app("usr-share", "other.desktop", "Other", "text/plain;");
        profile.write("config/mimeapps.list", "[Added Associations]\ntext/plain=chosen.desktop;\n[Default Applications]\ntext/plain=chosen.desktop;\n");
        let apps = MimeApps::new(profile.context(&[]));
        let default = apps
            .default_for("text/plain", &CancelToken::new())
            .unwrap()
            .unwrap();
        assert_eq!(default.app.id, "chosen.desktop");
        assert!(default.explicit);
    }

    // canonical_defaults_and_alias_declarations_share_one_specificity
    {
        let profile = Profile::new();
        profile.app("usr-share", "alias.desktop", "Alias", "text/x-enzui-alias;");
        profile.app("usr-share", "chosen.desktop", "Chosen", "text/plain;");
        profile.write("data/mime/aliases", "text/x-enzui-alias text/plain\n");
        profile.write(
            "config/mimeapps.list",
            "[Default Applications]\ntext/plain=chosen.desktop;\n",
        );
        let apps = MimeApps::new(profile.context(&[]));
        assert_eq!(
            candidate_ids(&apps, "text/plain"),
            ["alias.desktop", "chosen.desktop"]
        );
        let default = apps
            .default_for("text/x-enzui-alias", &CancelToken::new())
            .unwrap()
            .unwrap();
        assert_eq!(default.app.id, "chosen.desktop");
        assert!(default.explicit);
    }
}

#[test]
fn mime_handler_precedence_and_updates_preserve_effective_associations_4() {
    // setting_a_default_establishes_an_association_and_preserves_other_entries
    {
        let profile = Profile::new();
        profile.app("usr-share", "chosen.desktop", "Chosen", "image/png;");
        profile.write("config/mimeapps.list", "# retained comment\n[Default Applications]\nimage/png=other.desktop;\n[Added Associations]\ntext/plain=other.desktop;\n[Custom]\nSetting=retained\n");
        let apps = MimeApps::new(profile.context(&[]));
        assert!(candidate_ids(&apps, "text/plain").is_empty());
        apps.set_default("text/plain", "chosen.desktop", &CancelToken::new())
            .unwrap();
        let reread = MimeApps::new(profile.context(&[]));
        assert_eq!(candidate_ids(&reread, "text/plain"), ["chosen.desktop"]);
        let default = reread
            .default_for("text/plain", &CancelToken::new())
            .unwrap()
            .unwrap();
        assert_eq!(default.app.id, "chosen.desktop");
        assert!(default.explicit);
        let text = std::fs::read_to_string(profile.context(&[]).config_home.join("mimeapps.list"))
            .unwrap();
        assert!(text.contains("# retained comment\n"));
        assert!(text.contains("image/png=other.desktop;\n"));
        assert!(text.contains("Setting=retained\n"));
        let added = read_keyfile(&profile.context(&[]).config_home.join("mimeapps.list"))
            .unwrap()
            .unwrap();
        assert_eq!(
            split_list(&added["Added Associations"]["text/plain"]),
            ["other.desktop", "chosen.desktop"]
        );
    }

    // setting_a_default_reverses_its_removed_association
    {
        let profile = Profile::new();
        profile.app("usr-share", "chosen.desktop", "Chosen", "text/plain;");
        profile.write(
            "config/mimeapps.list",
            "[Removed Associations]\ntext/plain=chosen.desktop;other.desktop;\n",
        );
        let apps = MimeApps::new(profile.context(&[]));
        assert!(candidate_ids(&apps, "text/plain").is_empty());
        apps.set_default("text/plain", "chosen.desktop", &CancelToken::new())
            .unwrap();
        assert_eq!(candidate_ids(&apps, "text/plain"), ["chosen.desktop"]);
        let groups = read_keyfile(&profile.context(&[]).config_home.join("mimeapps.list"))
            .unwrap()
            .unwrap();
        assert_eq!(
            split_list(&groups["Removed Associations"]["text/plain"]),
            ["other.desktop"]
        );
    }

    // setting_an_alias_default_clears_equivalent_removals
    {
        let profile = Profile::new();
        profile.app("usr-share", "chosen.desktop", "Chosen", "image/png;");
        profile.write("data/mime/aliases", "text/x-enzui-alias text/plain\n");
        profile.write("config/mimeapps.list", "[Removed Associations]\ntext/plain=chosen.desktop;canonical.desktop;\ntext/x-enzui-alias=chosen.desktop;alias.desktop;\n");
        let apps = MimeApps::new(profile.context(&[]));
        assert!(candidate_ids(&apps, "text/plain").is_empty());
        apps.set_default("text/x-enzui-alias", "chosen.desktop", &CancelToken::new())
            .unwrap();
        for mime in ["text/plain", "text/x-enzui-alias"] {
            let default = apps
                .default_for(mime, &CancelToken::new())
                .unwrap()
                .unwrap();
            assert_eq!(default.app.id, "chosen.desktop");
            assert!(default.explicit);
            assert_eq!(candidate_ids(&apps, mime), ["chosen.desktop"]);
        }
        let groups = read_keyfile(&profile.context(&[]).config_home.join("mimeapps.list"))
            .unwrap()
            .unwrap();
        assert_eq!(
            split_list(&groups["Removed Associations"]["text/plain"]),
            ["canonical.desktop"]
        );
        assert_eq!(
            split_list(&groups["Removed Associations"]["text/x-enzui-alias"]),
            ["alias.desktop"]
        );
        assert_eq!(
            split_list(&groups["Added Associations"]["text/plain"]),
            ["chosen.desktop"]
        );
    }

    // concurrent_defaults_all_land_and_planted_links_are_not_followed
    {
        let profile = Profile::new();
        profile.app("usr-share", "chosen.desktop", "Chosen", "text/plain;");
        let config = profile.context(&[]).config_home;
        std::fs::create_dir_all(&config).unwrap();
        let victim = profile.root.path().join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        std::os::unix::fs::symlink(
            &victim,
            config.join(format!(".mimeapps.list.{}.tmp", std::process::id())),
        )
        .unwrap();

        let types: Vec<String> = (0..12)
            .map(|n| format!("application/x-enzui-{n}"))
            .collect();
        std::thread::scope(|scope| {
            for mime in &types {
                let apps = MimeApps::new(profile.context(&[]));
                scope.spawn(move || {
                    apps.set_default(mime, "chosen.desktop", &CancelToken::new())
                        .unwrap();
                });
            }
        });
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        let list = read_keyfile(&config.join("mimeapps.list"))
            .unwrap()
            .unwrap();
        for mime in &types {
            assert_eq!(
                list["Default Applications"].get(mime).map(String::as_str),
                Some("chosen.desktop;"),
                "{mime} was lost"
            );
        }
    }
}
