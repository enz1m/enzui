//! Desktop contrast and transparency preferences for sampled-backdrop materials.
//!
//! [`MaterialPreferenceFeed`] owns publication to the process-wide policy store.
//! Increased contrast and reduced transparency select opaque material fallback.
//! Reduced motion comes from iced’s existing transport and the live theme policy;
//! this backend does not add a second motion source.
//!
//! With `a11y-preferences-portal`, [`system_preferences`] reads the settings portal.
//! Its default contrast keys are `org.freedesktop.appearance/contrast`, then GNOME’s
//! `high-contrast` boolean. No transparency key is read until the host configures
//! `portal::PortalPreferenceBackend::with_transparency_key`; key names are desktop
//! policy. Without the feature, the source reports [`MaterialPreferenceRead::Unsupported`].
//!
//! An absent or unrecognized key leaves that dimension unanswered. Unsupported
//! sources release the platform layer; failed reads retain the last answer.
//! [`PreferenceFeedStatus`] reports the source outcome.

pub use enzui_theme::material::{
    MaterialPolicy, MaterialPolicyOwner, MaterialPolicySource, PreferenceSet, current_policy,
    install_policy_owner, policy_with_theme_motion, published_layers,
};
pub use enzui_theme::material_source::{
    DEFAULT_PREFERENCE_POLL_INTERVAL, FallbackPreferenceBackend, MaterialPreferenceBackend,
    MaterialPreferenceEvent, MaterialPreferenceFeed, MaterialPreferenceRead,
    MaterialPreferenceSignals, PreferenceBackendHandle, PreferenceFeedStatus, PreferenceSignal,
    ScriptedPreferenceBackend,
};

#[cfg(feature = "a11y-preferences-portal")]
pub mod portal;

/// Create the portal preference backend, or an unsupported source without its feature.
///
/// `a11y-preferences-portal` enables the transport. Otherwise reads return
/// [`MaterialPreferenceRead::Unsupported`] and the feed reports unsupported status.
#[must_use]
pub fn system_preferences() -> std::sync::Arc<dyn MaterialPreferenceBackend> {
    #[cfg(feature = "a11y-preferences-portal")]
    {
        std::sync::Arc::new(portal::PortalPreferenceBackend::new())
    }

    #[cfg(not(feature = "a11y-preferences-portal"))]
    {
        std::sync::Arc::new(NoPreferenceSource)
    }
}

/// Disable desktop preference input by reporting [`MaterialPreferenceRead::Unsupported`].
#[derive(Debug, Clone, Copy)]
pub struct NoPreferenceSource;

impl MaterialPreferenceBackend for NoPreferenceSource {
    fn identity(&self) -> String {
        "no-preference-source".to_owned()
    }

    fn read(&self) -> MaterialPreferenceRead {
        MaterialPreferenceRead::Unsupported
    }
}
