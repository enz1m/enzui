//! Run an iced program with enzUI runtime services and automatic focus traversal.
//!
//! [`run`] installs the default logger for framework diagnostics (see
//! [`enzui_core::logging`]; an application's own logger installed first wins),
//! the ordinary Wayland clipboard provider, the available
//! accessibility bridge and widget-runtime scope, and starts following the
//! process-wide [desktop settings](enzui_core::desktop_settings), before
//! entering the event loop. Default Tab and Shift+Tab traversal uses ignored key events, wraps in
//! the originating window and reveals focus through enclosing scroll
//! viewports. Captured keys stay with the widget.
//! Control, Alt and Logo chords remain available to application bindings.
//!
//! [`run_with_traversal`] accepts a custom decoder or disables automatic
//! decoding. A custom decoder is responsible for declining captured events and
//! chords it does not own. Hosts with their own event loop can compose
//! [`with_focus_traversal`] and arrange runtime services separately.
//!
//! Program configuration and user messages are forwarded. Boot presets are
//! dropped by the traversal wrapper and rejected by the run entry points.
//! [`run`] also applies assistive-technology actions (activate, focus, set
//! value, expand, set text selection) as they arrive, through
//! [`crate::a11y_actions`], so a screen reader can operate every widget with no
//! application code. An application that handles actions itself passes
//! [`AppServices::accessibility_actions`]`(false)` to [`run_with_services`] and
//! consumes the wake subscription from [`enzui_core::app_runtime`].

use enzui_core::focus_traversal;
use iced::Program;

pub use enzui_core::focus_traversal::{TraversalBinding, Traversing};

/// Adds automatic keyboard focus traversal to `program`.
///
/// The returned program publishes [`Traversing`] messages: `program`'s own
/// messages are forwarded unchanged, and a decoded traversal is answered with
/// the wrapping focus operation in the window that produced the input. Use it
/// when a host owns its own event loop; [`run`] is the ordinary entry point.
///
/// Boot presets of `program` are not carried over.
pub fn with_focus_traversal<P>(
    program: P,
    binding: TraversalBinding,
) -> impl Program<
    State = P::State,
    Message = Traversing<P::Message>,
    Theme = P::Theme,
    Renderer = P::Renderer,
> + 'static
where
    P: Program + 'static,
    P::Theme: 'static,
    P::Renderer: 'static,
{
    with_services(
        program,
        AppServices {
            traversal: binding,
            accessibility_actions: false,
            text_drag: false,
            localizer: None,
        },
    )
}

/// The runtime services [`run_with_services`] installs.
#[derive(Debug, Clone)]
pub struct AppServices {
    traversal: TraversalBinding,
    accessibility_actions: bool,
    text_drag: bool,
    localizer: Option<enzui_core::localization::Localizer>,
}

impl Default for AppServices {
    /// Default Tab traversal, and assistive-technology actions applied.
    fn default() -> Self {
        Self {
            traversal: TraversalBinding::default(),
            accessibility_actions: true,
            text_drag: true,
            localizer: None,
        }
    }
}

impl AppServices {
    /// Whether selected text drags out of fields and editors and dropped
    /// text goes into them ([`crate::text_drag`], `true` by default). The
    /// service owns the application's native drag connection; turn it off
    /// when the application connects `NativeDragRuntime` itself.
    #[must_use]
    pub fn text_drag(mut self, enabled: bool) -> Self {
        self.text_drag = enabled;
        self
    }

    /// Run the whole application in `localizer`'s scope: `boot`, `update`,
    /// `view`, `title` and `subscription`. The application's own messages
    /// then read through [`enzui_core::localization::text`] and
    /// [`localized!`](enzui_core::localized) anywhere on the UI thread, and
    /// framework controls follow the same language, with no app-side global
    /// or `tr` helper.
    ///
    /// Keep a clone to change the language: clones share their selection, so
    /// [`Localizer::select`](enzui_core::localization::Localizer::select) on
    /// it takes effect at the next update. Background work does not inherit
    /// the scope; pass it a clone.
    #[must_use]
    pub fn localizer(mut self, localizer: enzui_core::localization::Localizer) -> Self {
        self.localizer = Some(localizer);
        self
    }

    /// The keyboard traversal binding.
    #[must_use]
    pub fn traversal(mut self, binding: TraversalBinding) -> Self {
        self.traversal = binding;
        self
    }

    /// Whether assistive-technology actions are applied for the application
    /// (`true` by default). Turn it off when the application drains and
    /// applies them itself.
    #[must_use]
    pub fn accessibility_actions(mut self, apply: bool) -> Self {
        self.accessibility_actions = apply;
        self
    }
}

/// Adds `services` to `program`: keyboard traversal and, unless turned off,
/// applying assistive-technology actions. [`run_with_services`] runs the
/// result; a host with its own event loop runs it itself.
pub fn with_services<P>(
    program: P,
    services: AppServices,
) -> impl Program<
    State = P::State,
    Message = Traversing<P::Message>,
    Theme = P::Theme,
    Renderer = P::Renderer,
> + 'static
where
    P: Program + 'static,
    P::Theme: 'static,
    P::Renderer: 'static,
{
    struct FocusTraversing<P> {
        program: P,
        binding: TraversalBinding,
        accessibility_actions: bool,
        text_drag: bool,
        localizer: Option<enzui_core::localization::Localizer>,
        shadows: crate::window_frame_clip::ShadowSync,
        glass: crate::window_frame_clip::GlassSync,
    }

    impl<P> FocusTraversing<P> {
        /// Runs `operation` in the application's localizer scope, if it has one.
        fn localized<R>(&self, operation: impl FnOnce() -> R) -> R {
            match &self.localizer {
                Some(localizer) => localizer.scope(operation),
                None => operation(),
            }
        }
    }

    impl<P> Program for FocusTraversing<P>
    where
        P: Program,
        P::Theme: 'static,
        P::Renderer: 'static,
    {
        type State = P::State;
        type Message = Traversing<P::Message>;
        type Theme = P::Theme;
        type Renderer = P::Renderer;
        type Executor = P::Executor;

        fn name() -> &'static str {
            P::name()
        }

        fn settings(&self) -> iced::Settings {
            self.program.settings()
        }

        fn window(&self) -> Option<iced::window::Settings> {
            self.program.window()
        }

        fn boot(&self) -> (Self::State, iced::Task<Self::Message>) {
            let (state, task) = self.localized(|| self.program.boot());
            (state, task.map(Traversing::User))
        }

        fn update(
            &self,
            state: &mut Self::State,
            message: Self::Message,
        ) -> iced::Task<Self::Message> {
            let task = match message {
                Traversing::Accessibility(receipt) => {
                    crate::a11y_actions::apply_accessibility_wake(receipt)
                }
                Traversing::Service(message)
                    if self.glass.bound(&message) || self.glass.wanted(&message) =>
                {
                    iced::Task::none()
                }
                Traversing::Service(message) => {
                    crate::text_drag::update(&message).map(Traversing::Service)
                }
                message => self.localized(|| {
                    focus_traversal::reduce(message, |message| self.program.update(state, message))
                }),
            };
            // A theme the update changed reaches the window shadows, and a
            // window that now wants a glass frame is bound.
            iced::Task::batch([
                task,
                self.shadows.tasks(),
                self.glass.tasks(Traversing::Service),
            ])
        }

        fn view<'a>(
            &self,
            state: &'a Self::State,
            window: iced::window::Id,
        ) -> iced::Element<'a, Self::Message, Self::Theme, Self::Renderer> {
            let view = match &self.localizer {
                // The view wrapper keeps the scope through layout, events,
                // painting and accessibility, not only while building.
                Some(localizer) => {
                    localizer.view(|| self.program.view(state, window).map(Traversing::User))
                }
                None => self.program.view(state, window).map(Traversing::User),
            };
            let waiting = self
                .glass
                .saw(window, crate::window_frame_clip::take_wants_glass());
            // Always wrapped, so a window that turns to glass keeps its tree.
            let view = crate::desktop_glass::enz_desktop_glass(&self.glass.glass_of(window), view)
                .into_element();
            // A window waiting for its glass binding asks for the update that
            // binds it, instead of waiting (panes hidden) for the next input.
            let view = crate::window_frame_clip::nudge(
                view,
                waiting.then(|| -> Box<dyn Fn() -> Self::Message + 'a> {
                    Box::new(|| {
                        Traversing::Service(enzui_core::app_runtime::ServiceMessage::new(
                            crate::window_frame_clip::GlassWanted,
                        ))
                    })
                }),
            );
            let view = crate::window_frame_clip::host(crate::focus_claim::host(view));
            self.shadows
                .saw(window, crate::window_frame_clip::take_own_frame());
            if self.text_drag {
                crate::text_drag::host(view, window)
            } else {
                view
            }
        }

        fn title(&self, state: &Self::State, window: iced::window::Id) -> String {
            self.localized(|| self.program.title(state, window))
        }

        fn subscription(&self, state: &Self::State) -> iced::Subscription<Self::Message> {
            let subscription = focus_traversal::wrap_subscription(
                self.localized(|| self.program.subscription(state)),
                self.binding,
            );
            let subscription = if self.text_drag {
                iced::Subscription::batch([
                    subscription,
                    crate::text_drag::subscription().map(Traversing::Service),
                ])
            } else {
                subscription
            };
            if self.accessibility_actions {
                iced::Subscription::batch([
                    subscription,
                    enzui_core::app_runtime::accessibility_wake_subscription(
                        Traversing::Accessibility,
                    ),
                ])
            } else {
                subscription
            }
        }

        fn theme(&self, state: &Self::State, window: iced::window::Id) -> Option<Self::Theme> {
            self.program.theme(state, window)
        }

        fn style(&self, state: &Self::State, theme: &Self::Theme) -> iced::theme::Style {
            self.program.style(state, theme)
        }

        fn scale_factor(&self, state: &Self::State, window: iced::window::Id) -> f32 {
            self.program.scale_factor(state, window)
        }
    }

    FocusTraversing {
        program,
        binding: services.traversal,
        accessibility_actions: services.accessibility_actions,
        text_drag: services.text_drag,
        localizer: services.localizer,
        shadows: crate::window_frame_clip::ShadowSync::default(),
        glass: crate::window_frame_clip::GlassSync::default(),
    }
}

/// Run an iced program with runtime services and the default Tab traversal.
/// Traversal acts only on ignored keys in the originating window.
///
/// # Errors
///
/// Returns errors from the iced event loop.
///
/// # Panics
///
/// Panics if a different owner registered the process-global accessibility
/// bridge, or if the program carries boot presets the wrapper cannot forward.
pub fn run<P>(program: P) -> iced::Result
where
    P: Program + 'static,
    P::Theme: 'static,
    P::Renderer: 'static,
{
    run_with_traversal(program, TraversalBinding::default())
}

/// Run with a chosen traversal binding, including disabled or default bindings.
/// Other runtime services are installed as in [`run`].
///
/// # Errors
///
/// Returns errors from the iced event loop.
///
/// # Panics
///
/// Panics for the same reasons as [`run`].
pub fn run_with_traversal<P>(program: P, binding: TraversalBinding) -> iced::Result
where
    P: Program + 'static,
    P::Theme: 'static,
    P::Renderer: 'static,
{
    run_with_services(program, AppServices::default().traversal(binding))
}

/// Run with chosen [`AppServices`]: the traversal binding, and whether
/// assistive-technology actions are applied for the application.
///
/// # Errors
///
/// Returns errors from the iced event loop.
///
/// # Panics
///
/// Panics for the same reasons as [`run`].
pub fn run_with_services<P>(program: P, services: AppServices) -> iced::Result
where
    P: Program + 'static,
    P::Theme: 'static,
    P::Renderer: 'static,
{
    assert!(
        program.presets().is_empty(),
        "app_setup::run cannot forward boot presets: the traversal wrapper publishes \
         Traversing<Message>, and a Preset<State, Message> cannot be re-typed without \
         re-declaring it. Boot the program directly, or declare presets against the \
         wrapped message type."
    );
    run_program(with_services(program, services))
}

/// Run a program that already carries its services ([`with_services`]),
/// installing the rest of the runtime as [`run`] does: the default logger,
/// the clipboard provider, the desktop settings watcher and the
/// accessibility bridge.
///
/// This is the seam for wrapping the program once more before it runs, such
/// as iced's developer tools (see `docs/developer-tools.md`):
///
/// ```ignore
/// let program = app_setup::with_services(iced::application(..), AppServices::default());
/// #[cfg(feature = "devtools")]
/// let program = iced_devtools::attach(program);
/// app_setup::run_program(program)
/// ```
///
/// # Errors
///
/// Returns errors from the iced event loop.
///
/// # Panics
///
/// Panics if a different owner registered the process-global accessibility
/// bridge.
pub fn run_program<P>(program: P) -> iced::Result
where
    P: Program + 'static,
    P::Theme: 'static,
    P::Renderer: 'static,
{
    // Framework diagnostics reach stderr unless the application installed
    // its own logger first.
    let _ = enzui_core::logging::init_default();
    let _clipboard = enzui_clipboard_runtime::adopt_process_clipboard();
    // Desktop settings that act process-wide (middle-click paste) follow the
    // desktop from here on.
    enzui_core::desktop_settings::spawn_watcher();
    let outcome = enzui_core::app_runtime::run(|| Ok(iced_winit::run(program)?));
    // What the app copied stays pasteable after it exits.
    if let Err(error) = enzui_clipboard_runtime::keep_clipboard_after_exit() {
        log::warn!(target: "enzui::clipboard", "the copy could not be kept after exit: {error}");
    }
    outcome
}
