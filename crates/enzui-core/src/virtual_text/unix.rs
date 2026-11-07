//! AccessKit Unix provider adaptation, widget-to-window bindings, and accessibility-frame publication.

use super::*;
use iced_winit::accessibility::{self, virtual_text as adapter};

fn map_error(error: VirtualTextError) -> adapter::VirtualTextError {
    match error {
        VirtualTextError::WrongDocument => adapter::VirtualTextError::WrongDocument,
        VirtualTextError::StaleRevision { requested, current } => {
            adapter::VirtualTextError::StaleRevision {
                requested: requested.0,
                current: current.0,
            }
        }
        VirtualTextError::Cancelled => adapter::VirtualTextError::Cancelled,
        VirtualTextError::Range => adapter::VirtualTextError::Range,
        VirtualTextError::ByteBudget { requested, limit } => {
            adapter::VirtualTextError::ByteBudget { requested, limit }
        }
        VirtualTextError::CharacterBudget { requested, limit } => {
            adapter::VirtualTextError::CharacterBudget { requested, limit }
        }
        VirtualTextError::RunBudget { requested, limit } => {
            adapter::VirtualTextError::RunBudget { requested, limit }
        }
        VirtualTextError::ReplacementBudget { requested, limit } => {
            adapter::VirtualTextError::ReplacementBudget { requested, limit }
        }
        VirtualTextError::ReadOnly => adapter::VirtualTextError::ReadOnly,
        // Preserve timeout, full, closed, and reducer-refused as distinct adapter errors.
        VirtualTextError::Timeout => adapter::VirtualTextError::Timeout,
        VirtualTextError::MailboxFull => adapter::VirtualTextError::MailboxFull,
        VirtualTextError::MailboxClosed => adapter::VirtualTextError::MailboxClosed,
        VirtualTextError::ReducerRefused => adapter::VirtualTextError::ReducerRefused,
        VirtualTextError::StaleComposition
        | VirtualTextError::CompositionFinished
        | VirtualTextError::RevisionSequence { .. }
        | VirtualTextError::Unavailable => adapter::VirtualTextError::Unavailable,
    }
}

impl<B: VirtualTextBacking> adapter::VirtualTextProvider for VirtualTextDocument<B> {
    fn summary(&self) -> Result<adapter::VirtualTextSummary, adapter::VirtualTextError> {
        let summary = VirtualTextDocument::summary(self).map_err(map_error)?;
        Ok(adapter::VirtualTextSummary {
            document: summary.document.0,
            revision: summary.revision.0,
            bytes: summary.bytes,
            characters: summary.characters,
            selection_anchor: summary.selection.anchor,
            selection_focus: summary.selection.focus,
            read_only: summary.read_only,
        })
    }

    fn read(
        &self,
        document: u128,
        revision: u64,
        characters: Range<u64>,
        cancellation: &adapter::Cancellation,
    ) -> Result<adapter::VirtualTextRange, adapter::VirtualTextError> {
        if cancellation.is_cancelled() {
            return Err(adapter::VirtualTextError::Cancelled);
        }
        let linked_cancel = VirtualTextCancellation::from_accesskit_unix(cancellation);
        let runs = VirtualTextDocument::read(
            self,
            VirtualDocumentId(document),
            VirtualRevision(revision),
            characters.clone(),
            &linked_cancel,
        )
        .map_err(map_error)?;
        if cancellation.is_cancelled() {
            return Err(adapter::VirtualTextError::Cancelled);
        }
        let measured_bytes = runs.iter().map(|run| run.value.len()).sum();
        let measured_characters = runs.iter().map(|run| run.metrics.len()).sum();
        let mut value = String::with_capacity(measured_bytes);
        let mut character_lengths = Vec::with_capacity(measured_characters);
        for run in runs {
            value.push_str(&run.value);
            character_lengths.extend(run.metrics.into_iter().map(|metric| metric.utf8_bytes));
        }
        Ok(adapter::VirtualTextRange {
            document,
            revision,
            characters,
            value,
            character_lengths,
        })
    }

    fn set_selection(
        &self,
        document: u128,
        revision: u64,
        anchor: Option<u64>,
        focus: u64,
        cancellation: &adapter::Cancellation,
    ) -> Result<u64, adapter::VirtualTextError> {
        if cancellation.is_cancelled() {
            return Err(adapter::VirtualTextError::Cancelled);
        }
        let linked_cancel = VirtualTextCancellation::from_accesskit_unix(cancellation);
        let outcome = VirtualTextDocument::act(
            self,
            VirtualDocumentId(document),
            VirtualRevision(revision),
            VirtualTextAction::SetSelection(VirtualSelection { anchor, focus }),
            &linked_cancel,
        )
        .map_err(map_error)?;
        Ok(outcome.revision.0)
    }

    fn replace(
        &self,
        document: u128,
        revision: u64,
        characters: Range<u64>,
        value: &str,
        cancellation: &adapter::Cancellation,
    ) -> Result<u64, adapter::VirtualTextError> {
        if cancellation.is_cancelled() {
            return Err(adapter::VirtualTextError::Cancelled);
        }
        let linked_cancel = VirtualTextCancellation::from_accesskit_unix(cancellation);
        let outcome = VirtualTextDocument::act(
            self,
            VirtualDocumentId(document),
            VirtualRevision(revision),
            VirtualTextAction::Replace {
                range: characters,
                value: value.to_owned(),
            },
            &linked_cancel,
        )
        .map_err(map_error)?;
        Ok(outcome.revision.0)
    }
}

impl<B: VirtualTextBacking + 'static> VirtualTextDocument<B> {
    /// This document as the AT-SPI-facing provider object.
    pub fn accesskit_provider(self: &Arc<Self>) -> Arc<dyn adapter::VirtualTextProvider> {
        Arc::<Self>::clone(self)
    }

    /// Register this document through iced's public, window-scoped route.
    ///
    /// The application names the exact iced window, AccessKit tree and
    /// semantic node it publishes. The returned RAII handle owns the
    /// adapter registration; dropping it cancels pending provider work and
    /// withdraws provider-only interfaces. The document itself must remain
    /// strongly owned by the application because the mailbox deliberately
    /// retains only a weak provider reference.
    pub fn register_accessibility(
        self: &Arc<Self>,
        window: iced_core::window::Id,
        tree: accesskit::TreeId,
        node: accesskit::NodeId,
    ) -> accessibility::VirtualTextHandle {
        let provider = self.accesskit_provider();
        accessibility::register_virtual_text_provider(window, tree, node, &provider)
    }
}

// A widget binding acquires one provider handle for each window where that widget publishes a semantic node.
struct Binding {
    widget: iced_core::widget::Id,
    provider: std::sync::Weak<dyn adapter::VirtualTextProvider>,
    // Each live `(window, node, handle)` entry is withdrawn independently.
    bound: Vec<(
        iced_core::window::Id,
        accesskit::NodeId,
        accessibility::VirtualTextHandle,
    )>,
}

static BINDINGS: Mutex<Vec<Binding>> = Mutex::new(Vec::new());

/// Associates a document with the widget that publishes its accessibility node.
///
/// This may be called before any window exists and replaces an existing binding for the widget. The registry holds the document weakly and removes its window bindings on a later publish after the application drops it.
pub fn bind_provider_to_widget<B: VirtualTextBacking + 'static>(
    widget: iced_core::widget::Id,
    document: &Arc<VirtualTextDocument<B>>,
) {
    let provider: Arc<dyn adapter::VirtualTextProvider> =
        Arc::<VirtualTextDocument<B>>::clone(document);
    let weak = Arc::downgrade(&provider);
    drop(provider);
    if let Ok(mut bindings) = BINDINGS.lock() {
        bindings.retain(|binding| binding.widget != widget);
        bindings.push(Binding {
            widget,
            provider: weak,
            bound: Vec::new(),
        });
    }
}

/// Withdraw this widget's provider from every window.
pub fn unbind_provider_from_widget(widget: &iced_core::widget::Id) {
    if let Ok(mut bindings) = BINDINGS.lock() {
        bindings.retain(|binding| &binding.widget != widget);
    }
}

/// Reports providers published in a window and the subset already attached to its live adapter.
///
/// On the first publish, a provider may be registered but not yet bound until the adapter updates later in the same frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BindingResolution {
    /// Providers whose node was published in this frame's tree.
    pub registered: Vec<accesskit::NodeId>,
    /// Providers already attached to a live adapter for this window.
    pub bound: Vec<accesskit::NodeId>,
}

/// Updates provider handles for the currently publishing window.
///
/// Missing nodes lose only this window’s handle; dropped documents lose all handles.
pub fn resolve_bindings(
    window: iced_core::window::Id,
    tree: accesskit::TreeId,
    ids: &crate::collect::NodeIdMap,
) -> BindingResolution {
    let Ok(mut bindings) = BINDINGS.lock() else {
        return BindingResolution::default();
    };
    let mut resolution = BindingResolution::default();
    bindings.retain_mut(|binding| {
        let Some(provider) = binding.provider.upgrade() else {
            return false;
        };
        let Some(node) = ids.node_of(&binding.widget) else {
            // If this window publishes no node, withdraw only this window’s handle.
            binding.bound.retain(|(bound, _, _)| *bound != window);
            return true;
        };
        let existing = binding
            .bound
            .iter()
            .position(|(bound, _, _)| *bound == window);
        let live = existing.is_some_and(|at| {
            let (_, current, handle) = &binding.bound[at];
            *current == node && handle.is_bound()
        });
        if !live {
            // Drop a stale handle before registering its replacement because selectors permit only one live owner.
            if let Some(at) = existing {
                drop(binding.bound.remove(at));
            }
            binding.bound.push((
                window,
                node,
                accessibility::register_virtual_text_provider(window, tree, node, &provider),
            ));
        }
        resolution.registered.push(node);
        if binding
            .bound
            .iter()
            .any(|(bound, _, handle)| *bound == window && handle.is_bound())
        {
            resolution.bound.push(node);
        }
        true
    });
    resolution
}

/// Collects an accessibility frame and binds registered providers to the nodes published in it.
///
/// Returns the tree update and routable node map. `title` becomes the AT-SPI frame name.
pub fn collect_frame_binding_providers(
    window: iced_core::window::Id,
    title: Option<&str>,
    run: &mut dyn FnMut(&mut dyn iced_core::widget::Operation),
) -> (accesskit::TreeUpdate, crate::collect::NodeIdMap) {
    let mut collector = crate::collect::SemanticsCollector::new();
    run(&mut collector);
    let (update, ids) = collector.to_tree_update_for_window(window, title);
    let _resolution = resolve_bindings(window, update.tree_id, &ids);
    (update, ids)
}

/// Installs the process-global provider tree builder.
///
/// This entry point installs no action handler or window-close callback, publishes unnamed frames, and exclusively claims the accessibility bridge slot. Registration conflicts are logged; use [`crate::app_runtime::install_accessibility`] for the complete bridge.
pub fn install_provider_tree_builder<F>(on_frame: F)
where
    F: Fn(iced_core::window::Id, &crate::collect::NodeIdMap) + Send + Sync + 'static,
{
    if try_install_provider_tree_builder(on_frame).is_err() {
        log::error!(
            target: "enzui::a11y",
            "{PROVIDER_TREE_BUILDER_OWNER}: the process-global accessibility \
             tree builder already has an owner, so this registration was \
             refused. Install exactly one owner — ordinarily \
             enzui_core::app_runtime::run."
        );
    }
}

/// [`install_provider_tree_builder`], reporting a registration that was
/// refused because the bridge (or its tree-builder half) already has an
/// owner.
///
/// # Errors
///
/// `AlreadyRegistered` when the bridge or its tree builder already has an owner.
pub fn try_install_provider_tree_builder<F>(
    on_frame: F,
) -> Result<(), accessibility::AlreadyRegistered>
where
    F: Fn(iced_core::window::Id, &crate::collect::NodeIdMap) + Send + Sync + 'static,
{
    accessibility::try_set_tree_builder(move |window, run| {
        // This two-argument registration cannot supply titles; named frames require the `Bridge` installed by `crate::app_runtime`.
        let (update, ids) = collect_frame_binding_providers(window, None, run);
        on_frame(window, &ids);
        Some(update)
    })
}

/// Owner label used for provider tree-builder registration diagnostics.
pub const PROVIDER_TREE_BUILDER_OWNER: &str =
    "enzui_core::virtual_text::install_provider_tree_builder";

#[cfg(test)]
mod native_outcome_tests {
    use super::*;
    use std::time::Duration;

    fn document_with(mailbox: MutationMailbox) -> Arc<VirtualTextDocument<ChunkedTextBacking>> {
        Arc::new(VirtualTextDocument {
            document: VirtualDocumentId(7),
            budget: VirtualTextBudget::default(),
            mailbox: Arc::new(mailbox),
            state: Mutex::new(State {
                backing: BackingState::Ready(ChunkedTextBacking::new(TextStore::from_string(
                    "native".into(),
                ))),
                revision: VirtualRevision(11),
                selection: VirtualSelection::default(),
                read_only: false,
                next_composition: 1,
                composition: None,
                finished_composition: None,
            }),
            state_ready: Condvar::new(),
        })
    }

    fn set_selection_through_adapter(
        document: &Arc<VirtualTextDocument<ChunkedTextBacking>>,
        revision: u64,
        cancellation: &adapter::Cancellation,
    ) -> Result<u64, adapter::VirtualTextError> {
        let provider: Arc<dyn adapter::VirtualTextProvider> =
            Arc::<VirtualTextDocument<ChunkedTextBacking>>::clone(document);
        let _handle = accessibility::register_virtual_text_provider(
            iced_core::window::Id::unique(),
            accesskit::TreeId::ROOT,
            accesskit::NodeId(1),
            &provider,
        );
        provider.set_selection(7, revision, None, 1, cancellation)
    }

    #[test]
    fn each_application_refusal_survives_the_native_adapter_boundary() {
        let stale = document_with(MutationMailbox::new(2));
        let stale = set_selection_through_adapter(&stale, 10, &adapter::Cancellation::default())
            .unwrap_err();
        let cancelled = document_with(MutationMailbox::new(2));
        let cancellation = adapter::Cancellation::default();
        cancellation.cancel();
        let cancelled = set_selection_through_adapter(&cancelled, 11, &cancellation).unwrap_err();

        let timeout = document_with(MutationMailbox::with_reply_timeout(2, Duration::ZERO));
        let timeout =
            set_selection_through_adapter(&timeout, 11, &adapter::Cancellation::default())
                .unwrap_err();

        let full = document_with(MutationMailbox::new(1));
        let _queued = full
            .enqueue_mutation(
                VirtualDocumentId(7),
                VirtualRevision(11),
                VirtualTextAction::SetSelection(VirtualSelection::default()),
                &VirtualTextCancellation::default(),
            )
            .expect("the first request occupies the queue");
        let full = set_selection_through_adapter(&full, 11, &adapter::Cancellation::default())
            .unwrap_err();

        let closed = document_with(MutationMailbox::new(2));
        closed.mailbox().close();
        let closed = set_selection_through_adapter(&closed, 11, &adapter::Cancellation::default())
            .unwrap_err();

        let refused = document_with(MutationMailbox::new(2));
        let (woke, wake) = std::sync::mpsc::channel();
        let _registration = refused.mailbox().set_wake(move || {
            let _ = woke.send(());
        });
        let reducer_document = Arc::clone(&refused);
        let reducer = std::thread::spawn(move || {
            wake.recv().expect("enqueue wakes the reducer");
            for pending in reducer_document.mailbox().drain() {
                let _ = pending.resolve_with(|_| MutationReply::Refused);
            }
        });
        let refused =
            set_selection_through_adapter(&refused, 11, &adapter::Cancellation::default())
                .unwrap_err();
        reducer.join().expect("reducer thread");

        assert!(matches!(
            stale,
            adapter::VirtualTextError::StaleRevision {
                requested: 10,
                current: 11,
            }
        ));
        assert_eq!(cancelled, adapter::VirtualTextError::Cancelled);
        assert_eq!(timeout, adapter::VirtualTextError::Timeout);
        assert_eq!(full, adapter::VirtualTextError::MailboxFull);
        assert_eq!(closed, adapter::VirtualTextError::MailboxClosed);
        assert_eq!(refused, adapter::VirtualTextError::ReducerRefused);

        // Compare the adapter’s typed AT-SPI classes so distinct local errors remain distinct on D-Bus.
        let classes: Vec<adapter::AtspiErrorClass> =
            [&stale, &cancelled, &timeout, &full, &closed, &refused]
                .into_iter()
                .map(adapter::VirtualTextError::atspi_class)
                .collect();
        assert_eq!(
            classes,
            vec![
                adapter::AtspiErrorClass::Failed,
                adapter::AtspiErrorClass::NoReply,
                adapter::AtspiErrorClass::TimedOut,
                adapter::AtspiErrorClass::LimitsExceeded,
                adapter::AtspiErrorClass::Disconnected,
                adapter::AtspiErrorClass::AccessDenied,
            ],
            "each refusal must reach D-Bus as its own error class",
        );
        assert_eq!(
            classes
                .iter()
                .map(|class| class.dbus_name())
                .collect::<Vec<_>>(),
            vec![
                "Failed",
                "NoReply",
                "TimedOut",
                "LimitsExceeded",
                "Disconnected",
                "AccessDenied",
            ],
            "and under its own org.freedesktop.DBus.Error member name",
        );
        let distinct: std::collections::BTreeSet<adapter::AtspiErrorClass> =
            classes.iter().copied().collect();
        assert_eq!(distinct.len(), classes.len(), "no two may collapse");
    }
}
