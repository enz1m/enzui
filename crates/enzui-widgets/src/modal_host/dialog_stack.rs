/// How many dialogs a [`DialogStack`] shows at once: one modal host per level
/// stays mounted, so the page keeps its place in the tree whether zero or
/// three dialogs are open.
pub const DIALOG_STACK_DEPTH: usize = 3;

/// An event from one of a [`DialogStack`]'s hosts, to pass back to
/// [`DialogStack::update`].
#[derive(Debug, Clone)]
pub struct DialogEvent<Key> {
    key: Option<Key>,
    /// The showing of `key` the host belonged to, so an event that outlives
    /// it cannot act on a later showing of the same key.
    presentation: Option<enzui_core::modal::PresentationId>,
    event: ModalHostEvent,
}

impl<Key> DialogEvent<Key> {
    /// The dialog it came from; `None` from a level with no dialog open.
    #[must_use]
    pub fn key(&self) -> Option<&Key> {
        self.key.as_ref()
    }

    /// How the user asked to close the dialog, or `None` for focus moving
    /// within it. [`DialogStack::update`] closes the dialog when its policy
    /// permits the cause; an app that must refuse first (an operation in
    /// flight) sets the policy with [`DialogStack::set_policy`] instead.
    #[must_use]
    pub fn dismissal(&self) -> Option<DismissCause> {
        match self.event {
            ModalHostEvent::Dismissed(cause) => Some(cause),
            ModalHostEvent::FocusNext | ModalHostEvent::FocusPrevious => None,
        }
    }
}

/// The dialogs an application has open, topmost last, each with the focus it
/// gives back when it closes.
///
/// Open a dialog with [`Self::open`], render them all with [`Self::view`],
/// and pass each host's events to [`Self::update`]. A dialog's own buttons
/// (Save, Cancel, Close) close it with [`Self::dismiss`], which returns focus
/// to its invoker as Escape does. A dialog opened over another stacks on it;
/// closing a dialog closes the ones opened over it; [`Self::hand_off`]
/// replaces one dialog with the next (a list handing over to its edit form)
/// without returning focus in between. A covered dialog keeps its focus scope
/// and gets it back when it is uncovered.
///
/// ```ignore
/// // update
/// Message::OpenSettings(invoker) => return self.dialogs.open(Dialog::Settings, Some(invoker)),
/// Message::Dialog(event) => {
///     let (closed, focus) = self.dialogs.update(event);
///     for key in closed { self.forget(key) }
///     return focus;
/// }
/// Message::SaveSettings => {
///     let (closed, focus) = self.dialogs.dismiss(&Dialog::Settings, DismissCause::DefaultAction);
///     for key in closed { self.forget(key) }
///     return focus;
/// }
/// // view
/// self.dialogs.view(page, |key| self.dialog(key), Message::Dialog)
/// // with `enum Message { Dialog(DialogEvent<Dialog>), .. }`
/// ```
pub struct DialogStack<Key> {
    stack: enzui_core::modal::ModalStack<Key, ModalFocusLifecycle>,
}

impl<Key> std::fmt::Debug for DialogStack<Key> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialogStack")
            .field("depth", &self.stack.depth())
            .finish()
    }
}

impl<Key: Clone + PartialEq + std::fmt::Debug> Default for DialogStack<Key> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Key: Clone + PartialEq + std::fmt::Debug> DialogStack<Key> {
    /// No dialogs open.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stack: enzui_core::modal::ModalStack::new(),
        }
    }

    /// Open `key` over whatever is open, moving focus into it; `invoker` is
    /// the control focus returns to when it closes. Already open, or at
    /// [`DIALOG_STACK_DEPTH`], it does nothing.
    pub fn open<Message: Send + 'static>(
        &mut self,
        key: Key,
        invoker: Option<iced::advanced::widget::Id>,
    ) -> iced::Task<Message> {
        self.open_with(key, invoker, modal_dismiss_policy())
    }

    /// [`Self::open`] for a dialog that must be answered: Escape and the
    /// scrim do not close it.
    pub fn open_alert<Message: Send + 'static>(
        &mut self,
        key: Key,
        invoker: Option<iced::advanced::widget::Id>,
    ) -> iced::Task<Message> {
        self.open_with(key, invoker, alert_dismiss_policy())
    }

    fn open_with<Message: Send + 'static>(
        &mut self,
        key: Key,
        invoker: Option<iced::advanced::widget::Id>,
        policy: DismissPolicy,
    ) -> iced::Task<Message> {
        if self.stack.contains(&key) || self.stack.depth() >= DIALOG_STACK_DEPTH {
            return iced::Task::none();
        }
        let (lifecycle, focus) = ModalFocusLifecycle::open::<Message>(&format!("{key:?}"), invoker);
        if self.stack.present(key, policy, Some(lifecycle)).is_none() {
            return iced::Task::none();
        }
        iced::advanced::widget::operate(focus)
    }

    /// Close `from` and open `to` in its place, with `from`'s invoker: focus
    /// moves into `to` and returns to that invoker when `to` closes.
    pub fn hand_off<Message: Send + 'static>(
        &mut self,
        from: &Key,
        to: Key,
    ) -> iced::Task<Message> {
        let invoker = self
            .stack
            .iter()
            .find(|presentation| presentation.key() == from)
            .and_then(|presentation| presentation.restore_focus())
            .and_then(|lifecycle| lifecycle.invoker().cloned());
        // Moving on, as a dialog's default action does: always permitted.
        let _ = self.stack.dismiss(from, DismissCause::DefaultAction);
        self.open(to, invoker)
    }

    /// Apply an event from [`Self::view`]'s hosts. Returns the dialogs that
    /// closed (its dialog and any opened over it) and the focus work: focus
    /// moves within the dialog, or returns to the invoker once it closes. An
    /// event from a showing that has since closed (even if the same key was
    /// opened again) does nothing.
    pub fn update<Message: Send + 'static>(
        &mut self,
        event: DialogEvent<Key>,
    ) -> (Vec<Key>, iced::Task<Message>) {
        let DialogEvent {
            key,
            presentation,
            event,
        } = event;
        let (Some(key), Some(presentation)) = (key, presentation) else {
            return (Vec::new(), iced::Task::none());
        };
        let key = &key;
        if !self.stack.is_current_presentation(key, presentation) {
            return (Vec::new(), iced::Task::none());
        }
        let Some(lifecycle) = self
            .stack
            .iter()
            .find(|presentation| presentation.key() == key)
            .and_then(|presentation| presentation.restore_focus().cloned())
        else {
            return (Vec::new(), iced::Task::none());
        };
        let outcome = lifecycle.reduce::<Message>(event);
        let Some(cause) = outcome.dismissed() else {
            return (Vec::new(), outcome.into_task());
        };
        match self.stack.dismiss_checked(key, presentation, cause) {
            Ok(Some(unwound)) => {
                let mut closed: Vec<Key> = unwound
                    .orphaned
                    .into_iter()
                    .map(|resolution| resolution.key)
                    .collect();
                closed.push(unwound.resolved.key);
                (closed, outcome.into_task())
            }
            Ok(None) | Err(_) => (Vec::new(), iced::Task::none()),
        }
    }

    /// Close `key` (and the dialogs over it) from the app: a Save, Cancel or
    /// Close button, or a command that finishes the dialog's work. Returns
    /// the dialogs that closed and the task returning focus to `key`'s
    /// invoker. A cause the dialog's policy refuses (Escape on an alert)
    /// closes nothing; the explicit causes always close.
    pub fn dismiss<Message: Send + 'static>(
        &mut self,
        key: &Key,
        cause: DismissCause,
    ) -> (Vec<Key>, iced::Task<Message>) {
        let Some(lifecycle) = self
            .stack
            .iter()
            .find(|presentation| presentation.key() == key)
            .and_then(|presentation| presentation.restore_focus().cloned())
        else {
            return (Vec::new(), iced::Task::none());
        };
        match self.stack.dismiss(key, cause) {
            Some(unwound) => {
                let mut closed: Vec<Key> = unwound
                    .orphaned
                    .into_iter()
                    .map(|resolution| resolution.key)
                    .collect();
                closed.push(unwound.resolved.key);
                let focus = lifecycle
                    .reduce::<Message>(ModalHostEvent::Dismissed(cause))
                    .into_task();
                (closed, focus)
            }
            None => (Vec::new(), iced::Task::none()),
        }
    }

    /// Change whether `key` can be dismissed by Escape and the scrim: an
    /// [`alert_dismiss_policy`] while an operation it started is running, the
    /// [`modal_dismiss_policy`] again once it finishes. Its own buttons close
    /// it either way. Returns whether `key` is open.
    pub fn set_policy(&mut self, key: &Key, policy: DismissPolicy) -> bool {
        self.stack.set_policy(key, policy)
    }

    /// Close `key` (and the dialogs over it) because its subject went away,
    /// without moving focus.
    pub fn close(&mut self, key: &Key) -> Vec<Key> {
        // Backing out, as a cancel action does: always permitted.
        match self.stack.dismiss(key, DismissCause::CancelAction) {
            Some(unwound) => {
                let mut closed: Vec<Key> = unwound
                    .orphaned
                    .into_iter()
                    .map(|resolution| resolution.key)
                    .collect();
                closed.push(unwound.resolved.key);
                closed
            }
            None => Vec::new(),
        }
    }

    /// Whether `key` is open.
    #[must_use]
    pub fn is_open(&self, key: &Key) -> bool {
        self.stack.contains(key)
    }

    /// The topmost dialog.
    #[must_use]
    pub fn topmost(&self) -> Option<&Key> {
        self.stack
            .topmost()
            .map(enzui_core::modal::Presentation::key)
    }

    /// Whether no dialog is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// `page` under the open dialogs, each built by `dialog`. Hosts for
    /// every level stay mounted, so opening and closing never moves the page
    /// in the tree. A dialog leaves the tree when it closes.
    pub fn view<'a, Message: Clone + 'a>(
        &'a self,
        page: impl Into<Element<'a, Message>>,
        dialog: impl Fn(&Key) -> Element<'a, Message>,
        on_event: impl Fn(DialogEvent<Key>) -> Message + Clone + 'a,
    ) -> Element<'a, Message>
    where
        Key: 'a,
    {
        self.view_with(page, dialog, on_event, |_, host| host)
    }

    /// [`Self::view`], with each open dialog's host passed through
    /// `configure`: its [`placement`](EnzModalHost::placement) (a sheet at the
    /// bottom edge on a narrow window), [`label`](EnzModalHost::label) or
    /// [`alert`](EnzModalHost::alert) announcement.
    pub fn view_with<'a, Message: Clone + 'a>(
        &'a self,
        page: impl Into<Element<'a, Message>>,
        dialog: impl Fn(&Key) -> Element<'a, Message>,
        on_event: impl Fn(DialogEvent<Key>) -> Message + Clone + 'a,
        configure: impl Fn(&Key, EnzModalHost<'a, Message>) -> EnzModalHost<'a, Message>,
    ) -> Element<'a, Message>
    where
        Key: 'a,
    {
        let open: Vec<_> = self.stack.iter().collect();
        let mut view = page.into();
        for level in 0..DIALOG_STACK_DEPTH {
            let presentation = open.get(level);
            let key = presentation.map(|presentation| presentation.key().clone());
            let showing = presentation.map(|presentation| presentation.id());
            let on_event = on_event.clone();
            let mut host = enz_modal_host(
                view,
                presentation.map(|presentation| dialog(presentation.key())),
                move |event| {
                    on_event(DialogEvent {
                        key: key.clone(),
                        presentation: showing,
                        event,
                    })
                },
            );
            if let Some(presentation) = presentation {
                host = host.policy(presentation.policy());
                if let Some(lifecycle) = presentation.restore_focus() {
                    host = host.scoped_to(lifecycle);
                }
                host = configure(presentation.key(), host);
            }
            view = host.into();
        }
        view
    }
}
