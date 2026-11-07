//! The spelling back ends with a made-up Hunspell dictionary: read directly,
//! and through Enchant. Enchant reads its dictionary directories from the
//! environment at start, so its checks run in a child process of this test
//! binary with its own `XDG_CONFIG_HOME`; they are skipped where no Enchant
//! back end (libhunspell and the like) is installed.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::string_slice,
    clippy::print_stdout,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing"
)]
use enzui_core::spelling::{SpellCheck, SpellChecker};
use enzui_services::spell_check::{Enchant, Hunspell, SpellError, available_languages};
use std::path::{Path, PathBuf};
use std::process::Command;

const CHILD: &str = "ENZUI_SPELL_CHECK_CHILD";

/// Writes the `en_ZZ` dictionary into `dir`.
fn dictionary(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("en_ZZ.aff"),
        "SET UTF-8\nTRY esianrtolcdugmphbyfvkwzESIANRTOLCDUGMPHBYFVKWZ\n",
    )
    .unwrap();
    std::fs::write(dir.join("en_ZZ.dic"), "5\nthe\ncat\nsat\nmat\nhat\n").unwrap();
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("enzui-spell-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

/// Runs `test` in a child with a private Enchant configuration holding the
/// `en_ZZ` dictionary.
fn in_child(test: &str) {
    let root = scratch(test);
    dictionary(&root.join("enchant/hunspell"));
    let status = Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("XDG_CONFIG_HOME", &root)
        .status()
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);
    assert!(status.success(), "the child run of {test} failed");
}

fn enchant() -> Option<Enchant> {
    match Enchant::open("en_ZZ") {
        Ok(enchant) => Some(enchant),
        Err(SpellError::Unavailable(reason)) => {
            println!("skipped: {reason}");
            None
        }
        // Enchant loads, but none of its back ends does: it lists nothing.
        Err(_) if available_languages().is_ok_and(|tags| tags.is_empty()) => {
            println!("skipped: no Enchant back end is installed");
            None
        }
        Err(error) => panic!("{error}"),
    }
}

#[test]
fn words_are_checked_and_corrected() {
    if std::env::var_os(CHILD).is_none() {
        return in_child("words_are_checked_and_corrected");
    }
    let Some(enchant) = enchant() else { return };
    assert_eq!(enchant.language(), "en_ZZ");
    assert!(enchant.check("cat"));
    assert!(!enchant.check("caat"));
    let suggestions = enchant.suggest("caat");
    assert!(suggestions.iter().any(|s| s == "cat"), "{suggestions:?}");
    assert!(
        available_languages()
            .unwrap()
            .iter()
            .any(|tag| tag == "en_ZZ")
    );

    let check = SpellCheck::new(enchant);
    let text = "the caat sat on the matt";
    let found: Vec<&str> = check
        .misspelled(text)
        .into_iter()
        .map(|range| &text[range])
        .collect();
    assert_eq!(found, ["caat", "on", "matt"]);
}

#[test]
fn an_added_word_is_kept_in_the_personal_dictionary() {
    if std::env::var_os(CHILD).is_none() {
        return in_child("an_added_word_is_kept_in_the_personal_dictionary");
    }
    let Some(enchant) = enchant() else { return };
    assert!(!enchant.check("enzui"));
    enchant.add_to_dictionary("enzui");
    assert!(enchant.check("enzui"));
    drop(enchant);
    // A new dictionary, as another app would open it, knows the word.
    let reopened = Enchant::open("en_ZZ").unwrap();
    assert!(reopened.check("enzui"));
}

#[test]
fn a_missing_dictionary_is_an_error() {
    match Enchant::open("xx_NOPE") {
        Err(SpellError::NoDictionary(tag)) => assert_eq!(tag, "xx_NOPE"),
        Err(SpellError::Unavailable(reason)) => println!("skipped: {reason}"),
        Ok(_) => panic!("there is no xx_NOPE dictionary"),
    }
}

#[test]
fn hunspell_dictionaries_are_read_directly() {
    let root = scratch("hunspell");
    dictionary(&root.join("hunspell"));
    let personal = root.join("enchant/en_ZZ.dic");
    let dirs = [root.join("elsewhere"), root.join("hunspell")];
    let hunspell = Hunspell::open_in("en_ZZ", &dirs, Some(personal.clone())).unwrap();
    assert_eq!(hunspell.language(), "en_ZZ");
    assert!(hunspell.check("cat"));
    assert!(!hunspell.check("caat"));
    assert!(hunspell.suggest("caat").iter().any(|s| s == "cat"));

    let check = SpellCheck::new(hunspell);
    let text = "the caat sat on the matt";
    let found: Vec<&str> = check
        .misspelled(text)
        .into_iter()
        .map(|range| &text[range])
        .collect();
    assert_eq!(found, ["caat", "on", "matt"]);

    // An added word is written to Enchant's personal list and known on the
    // next open.
    check.add_to_dictionary("enzui");
    assert!(check.is_correct("enzui"));
    assert_eq!(std::fs::read_to_string(&personal).unwrap(), "enzui\n");
    let reopened = Hunspell::open_in("en_ZZ", &dirs, Some(personal)).unwrap();
    assert!(reopened.check("enzui"));
    assert!(matches!(
        Hunspell::open_in("de_DE", &dirs, None),
        Err(SpellError::NoDictionary(_))
    ));
    let _ = std::fs::remove_dir_all(&root);
}
