use super::*;
// Serialize tests that install owners of the process-wide policy store.
use crate::material::{MaterialPolicySource, current_policy, published_layers, store_lock};

fn scripted(reads: Vec<MaterialPreferenceRead>) -> Arc<dyn MaterialPreferenceBackend> {
    Arc::new(ScriptedPreferenceBackend::new("fixture", reads))
}

fn contrast_only(value: bool) -> PreferenceSet {
    PreferenceSet::NOTHING.with_increased_contrast(value)
}

#[test]
fn a_startup_read_publishes_the_desktops_answers_and_names_the_system() {
    let _guard = store_lock();
    let mut feed =
        MaterialPreferenceFeed::with_backend(scripted(vec![MaterialPreferenceRead::Delivered(
            contrast_only(true),
        )]));

    assert!(feed.is_enabled());
    assert_eq!(*feed.status(), PreferenceFeedStatus::AwaitingFirstRead);
    assert_eq!(
        current_policy(),
        MaterialPolicy::default(),
        "installing an owner must not publish anything on its own"
    );

    assert!(feed.update(MaterialPreferenceEvent::Initial(
        MaterialPreferenceRead::Delivered(contrast_only(true))
    )));

    let policy = current_policy();
    assert!(policy.increased_contrast);
    assert!(policy.forbids_material());
    assert_eq!(policy.source, MaterialPolicySource::System);
    assert!(
        !policy.reduced_transparency && !policy.reduced_motion,
        "a desktop that answered one key must not be read as answering three"
    );
    assert_eq!(
        *feed.status(),
        PreferenceFeedStatus::Delivered { answered: 1 }
    );
    assert_eq!(feed.generation(), 1);

    let (system, user) = published_layers();
    assert_eq!(system.increased_contrast, Some(true));
    assert_eq!(system.reduced_transparency, None);
    assert!(!user.is_answered());

    drop(feed);
    assert_eq!(current_policy(), MaterialPolicy::default());
}
