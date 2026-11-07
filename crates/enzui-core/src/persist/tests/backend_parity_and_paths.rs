use super::*;

#[test]
fn xdg_descendants_refuse_escape_before_real_atomic_save() {
    let root = TmpDir::new("xdg-descendants");
    let xdg = root.file("xdg");
    let outside = root.file("outside");
    std::fs::create_dir_all(&xdg).expect("create XDG root");
    std::fs::create_dir_all(&outside).expect("create outside sibling");
    let sentinel = outside.join("sentinel");
    let original = b"outside sentinel";
    std::fs::write(&sentinel, original).expect("seed outside sentinel");

    let resolver = XdgResolver::new(Some(&xdg), Some(&xdg), Some(&xdg), None::<&Path>);

    let valid = FileBackend::xdg_with_resolver(
        &resolver,
        PersistenceKind::State,
        "valid-app",
        "sessions/current.state",
        RawByteLimit::FRAMEWORK_MAX,
    )
    .expect("valid descendants")
    .expect("injected XDG root");
    valid
        .save_raw("descendant-gate", 1, "valid body")
        .expect("atomic save below XDG root");
    assert_eq!(
        valid.load_raw("descendant-gate", 1),
        BackendLoad::Loaded("valid body".to_owned())
    );
    assert_eq!(valid.path(), xdg.join("valid-app/sessions/current.state"));

    let hostile: &[(&str, &Path, XdgPathPart, XdgPathRejection)] = &[
        (
            "../outside",
            Path::new("sentinel"),
            XdgPathPart::Application,
            XdgPathRejection::NonNormalComponent,
        ),
        (
            "valid-app",
            sentinel.as_path(),
            XdgPathPart::Object,
            XdgPathRejection::NonNormalComponent,
        ),
        (
            "valid-app",
            Path::new("sessions/./current.state"),
            XdgPathPart::Object,
            XdgPathRejection::NonNormalComponent,
        ),
        (
            "valid-app",
            Path::new("sessions/../current.state"),
            XdgPathPart::Object,
            XdgPathRejection::NonNormalComponent,
        ),
        (
            "",
            Path::new("sentinel"),
            XdgPathPart::Application,
            XdgPathRejection::Empty,
        ),
        (
            "valid-app",
            Path::new(""),
            XdgPathPart::Object,
            XdgPathRejection::Empty,
        ),
        (
            "nul\0app",
            Path::new("sentinel"),
            XdgPathPart::Application,
            XdgPathRejection::InteriorNul,
        ),
        (
            "valid-app",
            Path::new("nul\0object"),
            XdgPathPart::Object,
            XdgPathRejection::InteriorNul,
        ),
    ];

    let mut accepted = Vec::new();
    for (app, object, expected_part, expected_rejection) in hostile {
        let outcome = FileBackend::xdg_with_resolver(
            &resolver,
            PersistenceKind::State,
            app,
            object,
            RawByteLimit::FRAMEWORK_MAX,
        );
        match outcome {
            Err(error) => {
                assert_eq!(
                    error.part(),
                    *expected_part,
                    "wrong input refused for {object:?}"
                );
                assert_eq!(
                    error.rejection(),
                    *expected_rejection,
                    "wrong reason for {object:?}"
                );
            }
            Ok(Some(backend)) => {
                backend
                    .save_raw("descendant-gate", 1, "escaped replacement")
                    .expect("an accepted path reaches the atomic writer");
                accepted.push(format!("{app:?}/{object:?}"));
            }
            Ok(None) => accepted.push(format!("{app:?}/{object:?} (missing root)")),
        }
    }

    assert_eq!(
        std::fs::read(&sentinel).expect("read outside sentinel"),
        original,
        "no refused descendant may replace the sibling sentinel"
    );
    assert_eq!(
        strict_inventory(&outside).expect("inventory outside sibling"),
        BTreeMap::from([(PathBuf::from("sentinel"), Some(original.to_vec()))]),
        "no escaped file or atomic staging residue may exist outside XDG"
    );
    assert!(
        accepted.is_empty(),
        "hostile descendants were accepted: {accepted:?}"
    );
}

fn strict_inventory(root: &Path) -> Result<BTreeMap<PathBuf, Option<Vec<u8>>>, String> {
    fn walk(
        root: &Path,
        directory: &Path,
        found: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
    ) -> Result<(), String> {
        let entries = std::fs::read_dir(directory).map_err(|error| {
            format!("open inventory directory {}: {error}", directory.display())
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!("read inventory entry in {}: {error}", directory.display())
            })?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|error| {
                    format!(
                        "inventory path {} escaped {}: {error}",
                        path.display(),
                        root.display()
                    )
                })?
                .to_path_buf();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                format!("read inventory metadata for {}: {error}", path.display())
            })?;
            if metadata.is_dir() {
                if found.insert(relative, None).is_some() {
                    return Err(format!("duplicate inventory path: {}", path.display()));
                }
                walk(root, &path, found)?;
            } else if metadata.is_file() {
                let data = std::fs::read(&path).map_err(|error| {
                    format!("read inventory data from {}: {error}", path.display())
                })?;
                if found.insert(relative, Some(data)).is_some() {
                    return Err(format!("duplicate inventory path: {}", path.display()));
                }
            } else {
                return Err(format!(
                    "inventory refuses symlink or special file: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    let mut found = BTreeMap::new();
    walk(root, root, &mut found)?;
    Ok(found)
}
