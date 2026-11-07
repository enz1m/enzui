//! Desktop font *preferences* and the precedence that turns them into a
//! [`FontCatalog`].
//!
//! Application overrides win over desktop families, followed by the supplied
//! theme catalog and each role's generic fallback. This does not discover or
//! load font files. Desktop size, weight, style and stretch remain available
//! through [`DesktopFontPreferences::parsed`] and do not change theme tokens.
//!
//! Resolve against a fresh theme catalog when preferences change; a catalog
//! already modified by this policy cannot recover its previous theme families.
//!
//! # Example
//!
//! ```
//! use enzui_theme::{FontCatalog, FontFamily, FontRole};
//! use enzui_theme::font_preferences::{DesktopFontPreferences, FontLayer, FontPreferencePolicy};
//!
//! let mut policy = FontPreferencePolicy::new();
//! policy.set_platform(Some(
//!     DesktopFontPreferences::new().with(FontRole::Ui, "Adwaita Sans 11"),
//! ));
//!
//! // The catalog handed in carries the theme-fallback layer.
//! let mut catalog = FontCatalog::default();
//! assert!(policy.apply(&mut catalog));
//! assert_eq!(
//!     catalog.family(FontRole::Ui),
//!     &FontFamily::Named("Adwaita Sans".to_owned()),
//! );
//!
//! // An explicit application override outranks the desktop.
//! policy.set_override(FontRole::Ui, Some(FontFamily::Named("Inter".to_owned())));
//! let resolved = policy.resolve(FontRole::Ui, &FontCatalog::default());
//! assert_eq!(resolved.layer, FontLayer::ApplicationOverride);
//! ```
//!
use crate::font_role::{
    FontCatalog, FontFamily, FontRole, ParsedFontDescription, RawFontDescription,
};

/// Which layer supplied the family bound to a role, highest precedence first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FontLayer {
    /// An explicit instance/application override.
    ApplicationOverride,
    /// The enabled desktop preference source.
    PlatformPreference,
    /// The family the theme itself carries (a theme file's `fonts.*` key).
    ThemeDocument,
    /// The role's generic family when the supplied theme already selects it.
    GenericFallback,
}

impl FontLayer {
    /// Every layer, in precedence order (highest first).
    pub const ALL: [FontLayer; 4] = [
        FontLayer::ApplicationOverride,
        FontLayer::PlatformPreference,
        FontLayer::ThemeDocument,
        FontLayer::GenericFallback,
    ];
}

/// A resolved role: the family to use and the layer that supplied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFontRole {
    /// The family the role resolves to.
    pub family: FontFamily,
    /// Which layer won.
    pub layer: FontLayer,
}

/// One desktop's per-role font choices, exactly as the platform delivered them.
///
/// A role with no entry means "the desktop said nothing about this role", which
/// is distinct from a role whose description is malformed: both fall through to
/// the lower layers, but only the second is a
/// [`malformed`](Self::malformed_roles) description worth reporting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesktopFontPreferences {
    ui: Option<RawFontDescription>,
    document: Option<RawFontDescription>,
    monospace: Option<RawFontDescription>,
}

impl DesktopFontPreferences {
    /// An empty snapshot: the desktop answered about no role at all.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder form of [`Self::set`], for a literal description.
    #[must_use]
    pub fn with(mut self, role: FontRole, description: impl Into<String>) -> Self {
        self.set(role, Some(RawFontDescription::new(description)));
        self
    }

    /// Records (or clears) the description the desktop gave for `role`.
    pub fn set(&mut self, role: FontRole, description: Option<RawFontDescription>) {
        *self.slot_mut(role) = description;
    }

    /// The untouched description for `role`, if the desktop supplied one.
    #[must_use]
    pub fn description(&self, role: FontRole) -> Option<&RawFontDescription> {
        self.slot(role)
    }

    /// The full Pango parse for `role`.
    ///
    /// Only [`family`](ParsedFontDescription::family) participates in
    /// precedence; size, weight, style and stretch are exposed here for a
    /// caller that deliberately wants them, and are never applied to the
    /// theme's semantic tokens by this module.
    #[must_use]
    pub fn parsed(&self, role: FontRole) -> Option<ParsedFontDescription> {
        self.slot(role)?.parse()
    }

    /// The family `role` resolves to, or `None` when the desktop said nothing
    /// or its description carries no family.
    #[must_use]
    pub fn family(&self, role: FontRole) -> Option<FontFamily> {
        self.slot(role)?.validate()
    }

    /// Whether the desktop answered about no role at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        FontRole::ALL.iter().all(|role| self.slot(*role).is_none())
    }

    /// The roles whose description was delivered but resolves to no family.
    ///
    /// These roles fall through to lower layers during resolution.
    #[must_use]
    pub fn malformed_roles(&self) -> Vec<FontRole> {
        FontRole::ALL
            .iter()
            .copied()
            .filter(|role| self.slot(*role).is_some_and(|raw| raw.validate().is_none()))
            .collect()
    }

    fn slot(&self, role: FontRole) -> Option<&RawFontDescription> {
        match role {
            FontRole::Ui => self.ui.as_ref(),
            FontRole::Document => self.document.as_ref(),
            FontRole::Monospace => self.monospace.as_ref(),
        }
    }

    fn slot_mut(&mut self, role: FontRole) -> &mut Option<RawFontDescription> {
        match role {
            FontRole::Ui => &mut self.ui,
            FontRole::Document => &mut self.document,
            FontRole::Monospace => &mut self.monospace,
        }
    }
}

/// The precedence owner: application overrides over an optional platform
/// snapshot, resolved against a theme-supplied catalog.
///
/// The catalog passed to [`resolve`](Self::resolve) / [`apply`](Self::apply)
/// supplies the two lowest layers, so it must be a catalog resolved from the
/// theme (document or built-in default) rather than one this policy already
/// wrote to. Applying to a freshly theme-resolved catalog is what makes a
/// disappearing desktop preference fall back instead of sticking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FontPreferencePolicy {
    ui_override: Option<FontFamily>,
    document_override: Option<FontFamily>,
    monospace_override: Option<FontFamily>,
    platform: Option<DesktopFontPreferences>,
}

impl FontPreferencePolicy {
    /// A policy with no overrides and no platform source.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets or clears the explicit override for `role`, the highest layer.
    ///
    /// Returns whether the stored override changed.
    pub fn set_override(&mut self, role: FontRole, family: Option<FontFamily>) -> bool {
        let slot = self.override_slot_mut(role);
        if *slot == family {
            return false;
        }
        *slot = family;
        true
    }

    /// The explicit override for `role`, if one is set.
    #[must_use]
    pub fn override_family(&self, role: FontRole) -> Option<&FontFamily> {
        self.override_slot(role)
    }

    /// Installs (or removes) the desktop snapshot.
    ///
    /// `None` means "no platform source is enabled or reachable" — the
    /// [`FontLayer::PlatformPreference`] layer is then skipped entirely, which
    /// is also what a desktop that disappears reduces to.
    ///
    /// Returns whether the stored snapshot changed.
    pub fn set_platform(&mut self, preferences: Option<DesktopFontPreferences>) -> bool {
        if self.platform == preferences {
            return false;
        }
        self.platform = preferences;
        true
    }

    /// The desktop snapshot currently in force.
    #[must_use]
    pub fn platform(&self) -> Option<&DesktopFontPreferences> {
        self.platform.as_ref()
    }

    /// Resolves `role` against `theme` (the two lowest layers).
    #[must_use]
    pub fn resolve(&self, role: FontRole, theme: &FontCatalog) -> ResolvedFontRole {
        if let Some(family) = self.override_slot(role) {
            return ResolvedFontRole {
                family: family.clone(),
                layer: FontLayer::ApplicationOverride,
            };
        }
        if let Some(family) = self.platform.as_ref().and_then(|p| p.family(role)) {
            return ResolvedFontRole {
                family,
                layer: FontLayer::PlatformPreference,
            };
        }
        let themed = theme.family(role).clone();
        // Generic provenance is inferred from the family, not document presence.
        let layer = if themed == FontRole::generic_font_family(role) {
            FontLayer::GenericFallback
        } else {
            FontLayer::ThemeDocument
        };
        ResolvedFontRole {
            family: themed,
            layer,
        }
    }

    /// Applies the full precedence to `catalog` in place.
    ///
    /// Returns whether any role's family changed. Repeated application is
    /// idempotent while the policy is unchanged; resolving a changed policy
    /// requires a fresh theme catalog to recover lower-layer families.
    pub fn apply(&self, catalog: &mut FontCatalog) -> bool {
        let mut changed = false;
        for role in FontRole::ALL {
            let resolved = self.resolve(role, catalog);
            changed |= catalog.set_family(role, resolved.family);
        }
        changed
    }

    fn override_slot(&self, role: FontRole) -> Option<&FontFamily> {
        match role {
            FontRole::Ui => self.ui_override.as_ref(),
            FontRole::Document => self.document_override.as_ref(),
            FontRole::Monospace => self.monospace_override.as_ref(),
        }
    }

    fn override_slot_mut(&mut self, role: FontRole) -> &mut Option<FontFamily> {
        match role {
            FontRole::Ui => &mut self.ui_override,
            FontRole::Document => &mut self.document_override,
            FontRole::Monospace => &mut self.monospace_override,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> FontFamily {
        FontFamily::Named(name.to_owned())
    }

    fn themed_catalog(role: FontRole, name: &str) -> FontCatalog {
        let mut catalog = FontCatalog::default();
        assert!(
            catalog.set_family(role, named(name)),
            "seeding the theme layer must be a real change"
        );
        catalog
    }

    #[test]
    fn a_disappearing_platform_returns_to_the_theme_family() {
        let mut policy = FontPreferencePolicy::new();
        policy.set_platform(Some(
            DesktopFontPreferences::new().with(FontRole::Ui, "Adwaita Sans 11"),
        ));
        let theme = themed_catalog(FontRole::Ui, "ThemeUi");

        let mut live = theme.clone();
        assert!(policy.apply(&mut live));
        assert_eq!(live.family(FontRole::Ui), &named("Adwaita Sans"));

        assert!(policy.set_platform(None));
        let mut after = theme;
        assert!(!policy.apply(&mut after));
        assert_eq!(after.family(FontRole::Ui), &named("ThemeUi"));
    }
}
