//! Pure policy for partitioning action descriptors between inline and overflow presentation.
//!
//! The budget counts inline slots. Invisible actions are omitted, pinned actions
//! remain inline, always-overflow actions remain in overflow, and lower priority
//! values fold first. Both outputs preserve authored order.

/// Preferred inline presentation, carried through without changing slot cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Presentation {
    /// Full inline presentation, such as icon and label.
    #[default]
    Expanded,
    /// Condensed inline presentation, such as icon-only or label-only.
    Compact,
}

/// Where the descriptor is allowed to appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverflowPolicy {
    /// Shows inline when space permits and otherwise folds; this is the default.
    #[default]
    FoldIfNeeded,
    /// Always remains inline, even when pinned actions exceed the budget.
    Pinned,
    /// Always appears in overflow, even when inline space remains.
    AlwaysOverflow,
}

/// Visibility rank where lower values fold first and authored order breaks ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct VisibilityPriority(pub u32);

/// Renderer-neutral action metadata consumed by the overflow resolver.
///
/// The stable `id` lets callers join the resolved descriptor back to their
/// typed action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    /// Stable key used to join this descriptor to its action.
    pub id: String,
    /// Placement policy for this descriptor.
    pub policy: OverflowPolicy,
    /// Lower folds first when budget runs out.
    pub priority: VisibilityPriority,
    /// Preferred presentation when shown inline. Metadata only.
    pub presentation: Presentation,
    /// Whether the action is enabled; disabled actions still occupy an inline slot.
    pub enabled: bool,
    /// Whether the descriptor participates; invisible descriptors appear in neither output.
    pub visible: bool,
}

impl Descriptor {
    /// Creates a visible, enabled, foldable descriptor with priority zero and expanded presentation.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            policy: OverflowPolicy::FoldIfNeeded,
            priority: VisibilityPriority(0),
            presentation: Presentation::Expanded,
            enabled: true,
            visible: true,
        }
    }

    /// Sets the visibility priority; lower values fold first.
    #[must_use]
    pub fn with_priority(mut self, priority: VisibilityPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Marks this descriptor as always inline.
    #[must_use]
    pub fn pinned(mut self) -> Self {
        self.policy = OverflowPolicy::Pinned;
        self
    }

    /// Builder: mark as always-overflow. The descriptor never appears inline.
    #[must_use]
    pub fn always_overflow(mut self) -> Self {
        self.policy = OverflowPolicy::AlwaysOverflow;
        self
    }

    /// Builder: set the preferred inline presentation. Metadata only.
    #[must_use]
    pub fn with_presentation(mut self, presentation: Presentation) -> Self {
        self.presentation = presentation;
        self
    }

    /// Builder: set the enabled flag. A disabled action still occupies its
    /// slot.
    #[must_use]
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets visibility; invisible descriptors appear in neither output.
    #[must_use]
    pub fn with_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }
}

/// Inline and overflow partitions, each preserving authored order and excluding invisible descriptors.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Descriptors that show inline, in authored order.
    pub inline: Vec<Descriptor>,
    /// Descriptors that fold into the overflow menu, in authored order.
    pub overflow: Vec<Descriptor>,
}

/// Partitions descriptors under an inline-slot budget.
///
/// Invisible descriptors are omitted. Always-overflow descriptors overflow;
/// pinned descriptors remain inline even beyond budget. Other descriptors fold
/// from lower priority upward, with earlier authored entries winning ties. Both
/// outputs preserve authored order. A zero budget folds every non-pinned action.
#[must_use]
pub fn resolve(descriptors: &[Descriptor], budget: usize) -> Plan {
    let commands: Vec<_> = descriptors
        .iter()
        .map(iced_core::layout::action_strip::Command::from)
        .collect();
    let plan = iced_core::layout::action_strip::resolve(&commands, budget);
    Plan {
        inline: plan
            .inline
            .into_iter()
            .map(|(index, _)| descriptors[index].clone())
            .collect(),
        overflow: plan
            .overflow
            .into_iter()
            .map(|index| descriptors[index].clone())
            .collect(),
    }
}

impl From<&Descriptor> for iced_core::layout::action_strip::Command {
    fn from(descriptor: &Descriptor) -> Self {
        use iced_core::layout::action_strip;
        Self {
            placement: match descriptor.policy {
                OverflowPolicy::FoldIfNeeded => action_strip::Placement::Fold,
                OverflowPolicy::Pinned => action_strip::Placement::Pinned,
                OverflowPolicy::AlwaysOverflow => action_strip::Placement::Overflow,
            },
            priority: descriptor.priority.0,
            visible: descriptor.visible,
            preferred: match descriptor.presentation {
                Presentation::Expanded => action_strip::Presentation::Expanded,
                Presentation::Compact => action_strip::Presentation::Compact,
            },
        }
    }
}
