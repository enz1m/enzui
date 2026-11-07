/// A widget focus stop or a tree row to restore after a context-menu event.
/// Tree rows share their tree's focus stop, so restoration also moves its cursor.
#[derive(Debug, Clone, PartialEq)]
pub enum FocusRestoreTarget {
    /// A control that is its own focus stop. Restored with
    /// [`enzui_core::focus::RestoreFocus`].
    Widget(Id),
    /// Focus the tree and select the supplied row.
    /// An ancestor path allows fallback when the exact row no longer exists.
    TreeRow {
        /// Tree identity that owns the focus stop.
        tree: crate::tree_view::TreeTarget,
        /// Desired row identity, if known.
        row: Option<Id>,
        /// Ancestor key path used when the desired row is gone.
        path: Option<Vec<String>>,
    },
}

impl FocusRestoreTarget {
    /// Restore to a control that is its own focus stop.
    #[must_use]
    pub fn widget(id: impl Into<Id>) -> Self {
        Self::Widget(id.into())
    }

    /// Restore to `row` of the tree named by `tree`.
    ///
    /// Prefer [`Self::tree_row_path`], which also supplies the ancestor path a
    /// removed row degrades along.
    #[must_use]
    pub fn tree_row(tree: crate::tree_view::TreeTarget, row: Option<Id>) -> Self {
        Self::TreeRow {
            tree,
            row,
            path: None,
        }
    }

    /// Derive a tree-row target with an ancestor path for fallback.
    /// [`Self::invoker_id`] exposes the same row identity for the host's AT address.
    #[must_use]
    pub fn tree_row_path(id_prefix: &str, path: &[&str]) -> Self {
        Self::TreeRow {
            tree: crate::tree_view::TreeTarget::prefix(id_prefix),
            row: Some(crate::tree_view::tree_row_id(id_prefix, path)),
            path: Some(path.iter().map(|s| (*s).to_owned()).collect()),
        }
    }

    /// Configured widget or tree-row identity for the host's invoker-derived AT address.
    /// None when a tree-row target was constructed without a row ID. This does
    /// not check whether that identity remains in the live tree.
    #[must_use]
    pub fn invoker_id(&self) -> Option<&Id> {
        match self {
            Self::Widget(id) => Some(id),
            Self::TreeRow { row, .. } => row.as_ref(),
        }
    }

    #[must_use]
    fn operation<T>(&self) -> Box<dyn Operation<T>>
    where
        T: Send + 'static,
    {
        match self {
            Self::Widget(id) => Box::new(enzui_core::focus::RestoreFocus::new(id.clone())),
            Self::TreeRow { tree, row, path } => {
                let op = crate::tree_view::focus_tree_row(tree.clone(), row.clone());
                Box::new(match path {
                    Some(path) => op.with_path(path.clone()),
                    None => op,
                })
            }
        }
    }
}

/// Map dismissal and activation event shapes to focus-restoration policy.
/// Host dismissal does not restore. This does not check staleness, availability
/// or whether resolving an activation actually closes the menu.
#[must_use]
pub fn event_restores_focus(event: &ContextMenuEvent) -> bool {
    match event {
        ContextMenuEvent::Dismissed(cause) => cause.restores_focus(),
        ContextMenuEvent::Activated { .. } | ContextMenuEvent::ActivatedAt { .. } => {
            CloseCause::Activated.restores_focus()
        }
        _ => false,
    }
}

/// Prepare restoration to a widget or tree-row target.
/// Returns None for non-restoring event shapes or a missing target. The caller
/// executes the returned operation; this helper does not validate menu lifecycle.
#[must_use]
pub fn focus_restore_operation_to<T>(
    event: &ContextMenuEvent,
    target: Option<&FocusRestoreTarget>,
) -> Option<Box<dyn Operation<T>>>
where
    T: Send + 'static,
{
    if !event_restores_focus(event) {
        return None;
    }
    Some(target?.operation())
}

/// A resolved command identity and an optional, unexecuted restoration operation.
pub struct ContextMenuUpdate<T> {
    activated: Option<EntryId>,
    restore: Option<Box<dyn Operation<T>>>,
}

impl<T> ContextMenuUpdate<T>
where
    T: Send + 'static,
{
    /// The live menu entry activated by this event, if any.
    #[must_use]
    pub fn activated(&self) -> Option<&EntryId> {
        self.activated.as_ref()
    }

    /// Batch activation and restoration tasks when restoration was prepared.
    /// The batch does not guarantee that activation completes before focus moves.
    pub fn finish(self, activation: iced::Task<T>) -> iced::Task<T> {
        match self.restore {
            Some(operation) => {
                iced::Task::batch([activation, iced::advanced::widget::operate(operation)])
            }
            None => activation,
        }
    }

    /// Split the result for harnesses or custom runtimes that execute widget
    /// operations directly instead of returning an iced task.
    #[must_use]
    pub fn into_parts(self) -> (Option<EntryId>, Option<Box<dyn Operation<T>>>) {
        (self.activated, self.restore)
    }
}
