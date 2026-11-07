//! Tagged file-chooser requests with a lazy backend, completion subscription and settlement.
//!
//! Call [`ChooserSession::start`] on a user gesture, return
//! [`ChooserSession::subscription`] from the app subscription, and call
//! [`ChooserSession::pump`] when it wakes. A session owns at most one request.
//! [`ChooserSession::cancel`] settles immediately and returns the tagged
//! cancellation without requiring another wake. No further pump delivers that request.
//!
//! [`ChooserSession::start_parented`] parents the dialog to a live window,
//! by an X11 handle or a Wayland export lease the session holds until the
//! request settles or is cancelled. A closed window or a dead export refuses
//! the request with [`PortalOutcome::ParentUnavailable`] rather than opening
//! an unparented dialog in its place.
//!
//! Forward each window close to [`ChooserSession::parent_closed`], which
//! cancels the request parented to that window as `ParentClosed`.
//! Dropping the session requests cancellation with [`CancelReason::Application`]
//! and settles before dropping its backend, without delivering an outcome.
//! Wire close is asynchronous; neither cancellation nor drop waits for the portal.
//! Messages already returned by `pump` belong to the application and cannot be
//! retracted; use application generation checks when handling those messages.

use super::fallback::{FallbackChooser, FallbackEvent, focus_task};
use super::{
    CancelReason, ChooserOptions, FileRequest, FileResult, PendingRequest, PortalBackend,
    PortalOperation, PortalOutcome, PortalParent, PortalWake,
};

/// The window a chooser request is parented to, or an explicit "none".
#[derive(Debug)]
pub enum ChooserParent {
    /// An unparented dialog, chosen on purpose.
    None,
    /// An encoded X11 parent for `window` (see
    /// [`export_parent`](crate::portal_parent::export_parent)).
    X11 {
        /// The window the handle names; it must still be live at start.
        window: iced::window::Id,
        /// Its `x11:` handle.
        handle: super::ParentHandle,
    },
    /// A Wayland xdg-foreign export of `window`, from
    /// [`export_parent_task`](crate::portal_parent::export_parent_task)'s
    /// [`ParentAdoption::Parented`](enzui_wayland::ParentAdoption::Parented).
    /// The session holds the lease until the request settles or is
    /// cancelled, then releases it.
    #[cfg(target_os = "linux")]
    Wayland {
        /// The exported window; it must still be live at start.
        window: iced::window::Id,
        /// The export lease.
        export: enzui_wayland::ParentExport,
    },
}

/// One app's file chooser: a lazily built backend and at most one request,
/// tagged with what it is for.
pub struct ChooserSession<Tag> {
    backend: Option<Box<dyn PortalBackend>>,
    /// The backend could not be built; later starts report `Unavailable`
    /// without trying again.
    unavailable: bool,
    pending: Option<(PendingRequest, Tag)>,
    /// What the live portal request asked for, kept so the in-app chooser
    /// can take over if the portal turns out to be unavailable.
    request: Option<(PortalOperation, ChooserOptions)>,
    /// Whether an unavailable portal opens the in-app chooser.
    fallback_enabled: bool,
    /// The open in-app chooser and the tag it answers for.
    fallback: Option<(FallbackChooser, Tag)>,
    /// Identifies the live request's completion subscription; bumped per
    /// request so a new dialog never inherits an old stream.
    generation: u64,
    /// The window the live request is parented to.
    parent_window: Option<iced::window::Id>,
    /// The Wayland export the live request is parented by, held while the
    /// request is unsettled.
    #[cfg(target_os = "linux")]
    lease: Option<enzui_wayland::ParentExport>,
}

impl<Tag> std::fmt::Debug for ChooserSession<Tag> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChooserSession")
            .field(
                "backend",
                &self.backend.as_ref().map(|backend| backend.describe()),
            )
            .field("unavailable", &self.unavailable)
            .field("pending", &self.pending.is_some())
            .field(
                "fallback",
                &self.fallback.as_ref().map(|(chooser, _)| chooser),
            )
            .field("generation", &self.generation)
            .finish()
    }
}

impl<Tag> Default for ChooserSession<Tag> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Tag> ChooserSession<Tag> {
    /// A session that builds the desktop portal backend on its first
    /// [`start`](Self::start). Without the `portal-dbus` feature there is no
    /// backend, and every start reports [`PortalOutcome::Unavailable`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            backend: None,
            unavailable: false,
            pending: None,
            request: None,
            fallback_enabled: false,
            fallback: None,
            generation: 0,
            parent_window: None,
            #[cfg(target_os = "linux")]
            lease: None,
        }
    }

    /// Releases the parent lease once no portal request needs it.
    fn release_lease(&mut self) {
        #[cfg(target_os = "linux")]
        if self.pending.is_none()
            && let Some(mut lease) = self.lease.take()
        {
            lease.release();
        }
    }

    /// Whether a parent lease is held for the live request.
    #[must_use]
    pub fn holds_parent_lease(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.lease.is_some()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// Answers requests with the in-app [`FallbackChooser`] whenever the
    /// portal is missing or does not implement FileChooser, instead of
    /// reporting [`PortalOutcome::Unavailable`].
    ///
    /// The app must then render [`view`](Self::view) around its content and
    /// route its events to [`handle`](Self::handle); without that the
    /// in-app dialog is never shown.
    #[must_use]
    pub fn with_fallback(mut self) -> Self {
        self.fallback_enabled = true;
        self
    }

    /// Whether the in-app chooser is open.
    #[must_use]
    pub fn is_fallback_open(&self) -> bool {
        self.fallback.is_some()
    }

    fn open_fallback(&mut self, tag: Tag, operation: PortalOperation, options: &ChooserOptions) {
        self.generation = self.generation.wrapping_add(1);
        self.fallback = Some((FallbackChooser::new(operation, options), tag));
    }

    /// Wraps `content` with the in-app chooser while it is open.
    ///
    /// `content` keeps its place in the widget tree whether the chooser is
    /// open or not, so its scroll offsets and editors survive the chooser.
    pub fn view<'a, Message: Clone + 'a>(
        &'a self,
        content: impl Into<iced::Element<'a, Message>>,
        on_event: impl Fn(FallbackEvent) -> Message + Clone + 'a,
    ) -> iced::Element<'a, Message> {
        super::fallback::host(
            self.fallback.as_ref().map(|(chooser, _)| chooser),
            content,
            on_event,
        )
    }

    /// Applies one in-app chooser event, delivering the tagged outcome when
    /// the user accepts or cancels.
    pub fn handle<Message: Send + 'static>(
        &mut self,
        event: FallbackEvent,
        on_outcome: impl FnOnce(Tag, PortalOutcome<FileResult>) -> Message,
    ) -> iced::Task<Message> {
        let focus = focus_task(&event);
        let Some((chooser, _)) = self.fallback.as_mut() else {
            return iced::Task::none();
        };
        let Some(outcome) = chooser.update(event) else {
            return focus;
        };
        match self.fallback.take() {
            Some((_, tag)) => iced::Task::done(on_outcome(tag, outcome)),
            None => iced::Task::none(),
        }
    }

    /// A session over a given backend, for tests and hosts with their own.
    #[must_use]
    pub fn with_backend(backend: Box<dyn PortalBackend>) -> Self {
        let mut session = Self::new();
        session.backend = Some(backend);
        session
    }

    /// Whether a request awaits completion or settlement, including a deferred start.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.fallback.is_some()
            || self
                .pending
                .as_ref()
                .is_some_and(|(pending, _)| pending.is_unsettled())
    }

    /// Cancels and settles the active request, returning its tag and cancellation once.
    ///
    /// Returns `None` when idle or when the terminal outcome was already claimed.
    /// An earlier recorded cancellation keeps its reason. The request is removed
    /// immediately, later replies cannot be delivered by this session, and a new
    /// request may start without another pump. A native close is requested at most
    /// once; the method does not wait for its acknowledgement.
    ///
    /// Outcomes already returned by [`Self::pump`] cannot be retracted.
    pub fn cancel(&mut self, reason: CancelReason) -> Option<(Tag, PortalOutcome<FileResult>)> {
        if let Some((_, tag)) = self.fallback.take() {
            return Some((tag, PortalOutcome::Cancelled(reason)));
        }
        let (mut pending, tag) = self.pending.take()?;
        let _ = pending.cancel(reason);
        if let Some(backend) = self.backend.as_mut() {
            pending.settle(&mut **backend);
        }
        self.release_lease();
        pending
            .claim_cancel_report()
            .map(|reason| (tag, PortalOutcome::Cancelled(reason)))
    }

    fn backend(&mut self) -> Option<&mut Box<dyn PortalBackend>> {
        if self.backend.is_none() && !self.unavailable {
            #[cfg(feature = "portal-dbus")]
            {
                self.backend = super::DbusBackend::new()
                    .ok()
                    .map(|backend| Box::new(backend) as Box<dyn PortalBackend>);
            }
            self.unavailable = self.backend.is_none();
        }
        self.backend.as_mut()
    }

    /// Starts an unparented dialog for `tag`, ignoring a start while another
    /// request is unsettled. [`Self::start_parented`] parents it to a window.
    ///
    /// # Errors
    ///
    /// The terminal outcome of a request that never started: no portal on
    /// this host, or a refusal before the dialog opened. Report it to the
    /// user like any other outcome.
    pub fn start(
        &mut self,
        tag: Tag,
        operation: PortalOperation,
        options: ChooserOptions,
    ) -> Result<(), PortalOutcome<FileResult>> {
        self.start_parented(tag, operation, options, ChooserParent::None, |_| true)
    }

    /// Starts a dialog for `tag` parented by `parent`, ignoring a start while
    /// another request is unsettled. `is_live` answers whether a window is
    /// still open (the app's own window list).
    ///
    /// A Wayland lease is held until the request settles or is cancelled, and
    /// released then. Without a portal, the in-app chooser opens as it does
    /// for [`Self::start`] and the lease is released at once.
    ///
    /// # Errors
    ///
    /// [`PortalOutcome::ParentUnavailable`] when the portal is asked to parent
    /// to a closed window or the export is no longer alive: the request is
    /// refused, never downgraded to an unparented dialog. Otherwise as
    /// [`Self::start`].
    pub fn start_parented(
        &mut self,
        tag: Tag,
        operation: PortalOperation,
        options: ChooserOptions,
        parent: ChooserParent,
        is_live: impl Fn(iced::window::Id) -> bool,
    ) -> Result<(), PortalOutcome<FileResult>> {
        if self.is_pending() {
            return Ok(());
        }
        let (policy, handle) = match &parent {
            ChooserParent::None => (PortalParent::None, super::ParentHandle::none()),
            ChooserParent::X11 { window, handle } => {
                (PortalParent::Window(*window), handle.clone())
            }
            #[cfg(target_os = "linux")]
            ChooserParent::Wayland { window, export } => {
                if !export.is_alive() {
                    return Err(PortalOutcome::ParentUnavailable);
                }
                (PortalParent::Window(*window), export.parent_handle())
            }
        };
        let Some(generation) = self.generation.checked_add(1) else {
            return Err(PortalOutcome::Unavailable);
        };
        let fallback_enabled = self.fallback_enabled;
        let Some(backend) = self.backend() else {
            if fallback_enabled {
                self.open_fallback(tag, operation, &options);
                return Ok(());
            }
            return Err(PortalOutcome::Unavailable);
        };
        let backend: &mut dyn PortalBackend = &mut **backend;
        let started = FileRequest::new(operation, policy)
            .with_parent_handle(handle)
            .with_options(options.clone())
            .start(backend, is_live);
        let pending = match started {
            Ok(pending) => pending,
            Err(PortalOutcome::Unavailable | PortalOutcome::Unsupported) if fallback_enabled => {
                self.open_fallback(tag, operation, &options);
                return Ok(());
            }
            Err(outcome) => return Err(outcome),
        };
        self.generation = generation;
        self.pending = Some((pending, tag));
        self.request = Some((operation, options));
        self.parent_window = match &parent {
            ChooserParent::None => None,
            ChooserParent::X11 { window, .. } => Some(*window),
            #[cfg(target_os = "linux")]
            ChooserParent::Wayland { window, .. } => Some(*window),
        };
        #[cfg(target_os = "linux")]
        if let ChooserParent::Wayland { export, .. } = parent {
            self.lease = Some(export);
        }
        Ok(())
    }

    /// Settles the live request as [`CancelReason::ParentClosed`] if it is
    /// parented to `window`; otherwise nothing. Forward every window close
    /// here, so a dialog never outlives the window it belongs to.
    pub fn parent_closed(
        &mut self,
        window: iced::window::Id,
    ) -> Option<(Tag, PortalOutcome<FileResult>)> {
        if self.pending.is_none() || self.parent_window != Some(window) {
            return None;
        }
        self.parent_window = None;
        self.cancel(CancelReason::ParentClosed)
    }

    /// The wake source for an unsettled request; no subscription when idle.
    pub fn subscription(&self) -> iced::Subscription<PortalWake> {
        if let Some((chooser, _)) = &self.fallback {
            return super::completion_signal_subscription(
                self.generation,
                chooser.signal().clone(),
            );
        }
        super::completion_subscription(
            self.generation,
            self.pending.as_ref().map(|(pending, _)| pending),
            self.backend.as_deref(),
        )
    }

    /// Advance the open dialog after a wake. Delivers the outcome, with the
    /// tag its request was started for, exactly once.
    pub fn pump<Message: Send + 'static>(
        &mut self,
        on_outcome: impl FnOnce(Tag, PortalOutcome<FileResult>) -> Message,
    ) -> iced::Task<Message>
    where
        Tag: Clone,
    {
        if let Some((chooser, _)) = self.fallback.as_mut() {
            chooser.absorb();
            return iced::Task::none();
        }
        let (Some((pending, tag)), Some(backend)) = (self.pending.as_mut(), self.backend.as_mut())
        else {
            return iced::Task::none();
        };
        let tag = tag.clone();
        let outcome = super::pump_outcome(pending, &mut **backend);
        if !pending.is_unsettled() {
            self.pending = None;
            self.release_lease();
        }
        match outcome {
            Some(PortalOutcome::Unavailable | PortalOutcome::Unsupported)
                if self.fallback_enabled
                    && let Some((operation, options)) = self.request.take() =>
            {
                self.open_fallback(tag, operation, &options);
                iced::Task::none()
            }
            Some(outcome) => {
                self.request = None;
                iced::Task::done(on_outcome(tag, outcome))
            }
            None => iced::Task::none(),
        }
    }
}

impl<Tag> Drop for ChooserSession<Tag> {
    fn drop(&mut self) {
        let _ = self.cancel(CancelReason::Application);
    }
}

#[cfg(test)]
mod tests;
