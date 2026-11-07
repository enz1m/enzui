//! Desktop-notification client state and reducer delivery.
//!
//! Notifications use a selected desktop transport independently of in-window toasts.
//! The XDG portal remains available through `portal-dbus`; the opt-in
//! `notification-freedesktop` feature exposes `freedesktop::FreedesktopNotificationBackend`
//! for direct desktop services with negotiated inline replies.
//! A caller submits a validated [`DesktopNotification`], includes
//! [`DesktopNotificationClient::wake_subscription`] in its iced subscriptions,
//! and drains [`NotificationEvent`] values from its reducer after each wake.
//! Reusing a [`NotificationId`] updates the existing desktop notification.
//! External services remain responsible for display, history, DND, and
//! lock-screen policy.
//!
//! A duplicate id is an update. A process restart starts with no trusted action
//! registrations, so the application re-sends each durable id before callbacks
//! for it are accepted. Service-owner replacement invalidates all prior action
//! bindings. Dropping a client leaves posted notifications intact; use
//! [`NotificationShutdown::Withdraw`] for explicit removal during shutdown.
//!
//! ```
//! use enzui_core::app_data::AppId;
//! use enzui_core::portal::notification::{
//!     DesktopNotification, DesktopNotificationClient, NotificationId,
//!     UnavailableNotificationBackend,
//! };
//!
//! let app_id = AppId::new("org.example.Reader")?;
//! let notice_id = NotificationId::new("download-complete")?;
//! let mut notifications = DesktopNotificationClient::new(
//!     app_id,
//!     UnavailableNotificationBackend,
//! );
//! let outcome = notifications.send(DesktopNotification::new(notice_id, "Download complete"));
//! assert!(outcome.is_err());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use iced_futures::Subscription;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::app_data::AppId;
use crate::portal::completion::CompletionSignal;

#[cfg(feature = "notification-freedesktop")]
pub mod freedesktop;

/// Maximum UTF-8 byte length of an application-local notification id.
pub const MAX_NOTIFICATION_ID_BYTES: usize = 128;
/// Maximum UTF-8 byte length of a notification title.
pub const MAX_NOTIFICATION_TITLE_BYTES: usize = 256;
/// Maximum UTF-8 byte length of a plain or markup body.
pub const MAX_NOTIFICATION_BODY_BYTES: usize = 16 * 1024;
/// Maximum number of buttons retained for one notification.
pub const MAX_NOTIFICATION_BUTTONS: usize = 8;
/// Maximum bytes retained for an inline icon.
pub const MAX_NOTIFICATION_ICON_BYTES: usize = 256 * 1024;
/// Maximum active notification identities retained by one client.
pub const MAX_ACTIVE_NOTIFICATIONS: usize = 128;
/// Maximum accepted commands awaiting a terminal backend event.
pub const MAX_PENDING_NOTIFICATION_OPERATIONS: usize = 64;
/// Maximum UTF-8 byte length of an action target or response.
pub const MAX_NOTIFICATION_ACTION_VALUE_BYTES: usize = 4 * 1024;
/// Maximum action callbacks retained by a transport before reducer drain.
pub const NOTIFICATION_ACTION_QUEUE_CAPACITY: usize = 64;
/// Maximum encoded body bytes accepted for a single `ActionInvoked` signal.
///
/// The bound covers two identifiers plus a target, an activation token, and a
/// user response, each capped by the typed field limits above, with headroom
/// for the `Vec<OwnedValue>` framing.
pub const MAX_NOTIFICATION_ACTION_WIRE_BYTES: usize =
    2 * MAX_NOTIFICATION_ID_BYTES + 4 * MAX_NOTIFICATION_ACTION_VALUE_BYTES + 512;
/// Maximum extra body elements accepted after the notification and action ids.
pub const MAX_NOTIFICATION_ACTION_PARAMETERS: usize = 4;

/// A stable application-local notification identifier.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NotificationId(String);

impl NotificationId {
    /// Validates an application-local notification id.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationValidationError::InvalidNotificationId`] when the
    /// value is empty, oversized, contains control characters, or contains a
    /// path separator.
    pub fn new(value: impl Into<String>) -> Result<Self, NotificationValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_NOTIFICATION_ID_BYTES
            || value.chars().any(char::is_control)
            || value.contains('/')
            || value.contains('\\')
        {
            return Err(NotificationValidationError::InvalidNotificationId);
        }
        Ok(Self(value))
    }

    /// Returns the validated application-local id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for NotificationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NotificationId").field(&self.0).finish()
    }
}

impl fmt::Display for NotificationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A named action delivered through the portal signal path.
///
/// Names beginning with `app.` are rejected because the portal routes those
/// through `org.freedesktop.Application.ActivateAction` instead of this
/// client's reducer queue.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NotificationActionId(String);

impl NotificationActionId {
    /// Validates an action name for reducer delivery.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationValidationError::InvalidActionId`] for an empty,
    /// oversized, control-containing, or `app.`-prefixed name.
    pub fn new(value: impl Into<String>) -> Result<Self, NotificationValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_NOTIFICATION_ID_BYTES
            || value.starts_with("app.")
            || value.chars().any(char::is_control)
        {
            return Err(NotificationValidationError::InvalidActionId);
        }
        Ok(Self(value))
    }

    /// Returns the validated action name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for NotificationActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NotificationActionId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for NotificationActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A bounded action target understood by the framework transport.
#[derive(Clone, PartialEq, Eq)]
pub enum NotificationActionTarget {
    /// UTF-8 application data.
    Text(String),
    /// Signed numeric application data.
    Integer(i64),
    /// Unsigned numeric application data.
    Unsigned(u64),
    /// Boolean application data.
    Boolean(bool),
    /// Opaque application bytes.
    Bytes(Vec<u8>),
}

impl NotificationActionTarget {
    fn validate(&self) -> Result<(), NotificationValidationError> {
        let bytes = match self {
            Self::Text(value) if value.contains('\0') => {
                return Err(NotificationValidationError::InvalidActionValue);
            }
            Self::Text(value) => value.len(),
            Self::Bytes(value) => value.len(),
            Self::Integer(_) | Self::Unsigned(_) | Self::Boolean(_) => 0,
        };
        if bytes > MAX_NOTIFICATION_ACTION_VALUE_BYTES {
            Err(NotificationValidationError::ActionValueTooLarge)
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for NotificationActionTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(value) => f.debug_struct("Text").field("bytes", &value.len()).finish(),
            Self::Bytes(value) => f
                .debug_struct("Bytes")
                .field("bytes", &value.len())
                .finish(),
            Self::Integer(_) => f.write_str("Integer(<redacted>)"),
            Self::Unsigned(_) => f.write_str("Unsigned(<redacted>)"),
            Self::Boolean(_) => f.write_str("Boolean(<redacted>)"),
        }
    }
}

/// Desktop notification priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NotificationPriority {
    /// Lower than ordinary traffic.
    Low,
    /// Ordinary priority.
    #[default]
    Normal,
    /// Important but not urgent.
    High,
    /// Urgent notification.
    Urgent,
}

/// A portal display hint; the desktop remains free to apply its own policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationDisplayHint {
    /// Show only as a transient banner.
    Transient,
    /// Put the notification in the desktop's tray or history without a banner.
    Tray,
    /// Keep the notification while the application has a window.
    Persistent,
    /// Suppress the notification on the lock screen.
    HideOnLockScreen,
    /// Hide notification content on the lock screen.
    HideContentOnLockScreen,
    /// Present an update as new.
    ShowAsNew,
}

/// Icon data accepted by the portal client.
#[derive(Clone, PartialEq, Eq)]
pub enum NotificationIcon {
    /// Freedesktop icon-theme names in preference order.
    Themed(Vec<String>),
    /// Encoded PNG, JPEG, or SVG bytes.
    ///
    /// The portal backend checks image headers and square dimensions before
    /// submission, then sends bytes to version 1 or a sealed, close-on-exec
    /// memory descriptor to version 2. Raster edges must be at most 512 pixels;
    /// SVG must be UTF-8 XML without a DTD and fit in 4096 bytes. The encoded
    /// limit is 256 KiB for either format. Invalid headers or dimensions produce an operation failure;
    /// the desktop performs final image-content validation. The direct
    /// freedesktop backend reports this icon form as unsupported.
    Bytes(Vec<u8>),
}

impl fmt::Debug for NotificationIcon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Themed(names) => f.debug_tuple("Themed").field(names).finish(),
            Self::Bytes(bytes) => f
                .debug_struct("Bytes")
                .field("bytes", &bytes.len())
                .finish(),
        }
    }
}

/// A notification activation with reducer or explicit exported delivery.
#[derive(Clone, PartialEq, Eq)]
pub struct NotificationAction {
    id: NotificationActionId,
    target: Option<NotificationActionTarget>,
    exported: bool,
}

impl NotificationAction {
    /// Creates a reducer callback without a target.
    #[must_use]
    pub fn new(id: NotificationActionId) -> Self {
        Self {
            id,
            target: None,
            exported: false,
        }
    }

    /// Whether the desktop invokes an exported application action instead of the reducer.
    #[must_use]
    pub fn is_exported(&self) -> bool {
        self.exported
    }

    pub(crate) fn wire_name(&self) -> std::borrow::Cow<'_, str> {
        if self.exported {
            std::borrow::Cow::Owned(format!("app.{}", self.id.as_str()))
        } else {
            std::borrow::Cow::Borrowed(self.id.as_str())
        }
    }

    /// Attaches the target delivered with this activation.
    #[must_use]
    pub fn target(mut self, target: NotificationActionTarget) -> Self {
        self.target = Some(target);
        self
    }

    /// The logical action name, without the wire prefix for exported actions.
    #[must_use]
    pub fn id(&self) -> &NotificationActionId {
        &self.id
    }

    /// The target delivered to the reducer or exported application action.
    #[must_use]
    pub fn action_target(&self) -> Option<&NotificationActionTarget> {
        self.target.as_ref()
    }
}

impl fmt::Debug for NotificationAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotificationAction")
            .field("id", &self.id)
            .field("exported", &self.exported)
            .field("has_target", &self.target.is_some())
            .finish()
    }
}

/// An exported application action that can reactivate the app after it exits.
///
/// The portal sends `app.<name>` and the desktop calls
/// `org.freedesktop.Application.ActivateAction` with the bare name, target and
/// platform context. Install a matching desktop entry and D-Bus activation
/// service, and serve the action through `crate::activation::dbus::ApplicationService`
/// (feature `application-dbus`). No reducer callback is registered.
#[derive(Clone, PartialEq, Eq)]
pub struct NotificationExportedAction(NotificationAction);

impl NotificationExportedAction {
    /// Validates a bare exported name without the portal's `app.` prefix.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationValidationError::InvalidActionId`] unless the name
    /// is nonempty ASCII alphanumeric, hyphen or period, has no `app.` prefix,
    /// and fits the action-id bound including the four-byte wire prefix.
    pub fn new(name: impl Into<String>) -> Result<Self, NotificationValidationError> {
        let name = name.into();
        if name.len().saturating_add(4) > MAX_NOTIFICATION_ID_BYTES
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-.".contains(&byte))
        {
            return Err(NotificationValidationError::InvalidActionId);
        }
        let mut action = NotificationAction::new(NotificationActionId::new(name)?);
        action.exported = true;
        Ok(Self(action))
    }

    /// Adds the value passed to `ActivateAction` before any reply text.
    #[must_use]
    pub fn target(mut self, target: NotificationActionTarget) -> Self {
        self.0 = self.0.target(target);
        self
    }

    /// The logical exported name, without the portal prefix.
    #[must_use]
    pub fn id(&self) -> &NotificationActionId {
        self.0.id()
    }
}

impl From<NotificationExportedAction> for NotificationAction {
    fn from(action: NotificationExportedAction) -> Self {
        action.0
    }
}

impl fmt::Debug for NotificationExportedAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NotificationExportedAction")
            .field(&self.0)
            .finish()
    }
}

/// An optional semantic purpose for a notification button.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NotificationButtonPurpose(String);

impl NotificationButtonPurpose {
    /// Validates a standard or vendor-prefixed portal purpose.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationValidationError::InvalidPurpose`] for an empty,
    /// oversized, or non-ASCII identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, NotificationValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_NOTIFICATION_ID_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
        {
            return Err(NotificationValidationError::InvalidPurpose);
        }
        Ok(Self(value))
    }

    /// Returns the portal purpose string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for NotificationButtonPurpose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NotificationButtonPurpose")
            .field(&self.0)
            .finish()
    }
}

/// A labelled notification button.
#[derive(Clone, PartialEq, Eq)]
pub struct NotificationButton {
    label: String,
    action: NotificationAction,
    purpose: Option<NotificationButtonPurpose>,
}

impl NotificationButton {
    /// Creates a button with a visible label and callback or exported action.
    #[must_use]
    pub fn new(label: impl Into<String>, action: impl Into<NotificationAction>) -> Self {
        Self {
            label: label.into(),
            action: action.into(),
            purpose: None,
        }
    }

    /// Adds an optional service-advertised purpose.
    #[must_use]
    pub fn purpose(mut self, purpose: NotificationButtonPurpose) -> Self {
        self.purpose = Some(purpose);
        self
    }

    /// The visible label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The callback or exported action.
    #[must_use]
    pub fn action(&self) -> &NotificationAction {
        &self.action
    }

    /// The optional semantic purpose.
    #[must_use]
    pub fn button_purpose(&self) -> Option<&NotificationButtonPurpose> {
        self.purpose.as_ref()
    }
}

impl fmt::Debug for NotificationButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotificationButton")
            .field("label_bytes", &self.label.len())
            .field("action", &self.action)
            .field("purpose", &self.purpose)
            .finish()
    }
}

/// Inline reply delivered to a callback or an exported application action.
///
/// Direct freedesktop transport requires both `actions` and `inline-reply`.
/// Portal version 2 requires advertised `im.received` and `im.reply-with-text`
/// support; an explicit category must be `im.received`. The portal uses the
/// submit label but leaves input presentation and placeholder text to the desktop.
#[derive(Clone, PartialEq, Eq)]
pub struct NotificationInlineReply {
    action: NotificationAction,
    placeholder: String,
    submit_label: String,
}

impl NotificationInlineReply {
    /// Creates an inline reply with localized input and submit labels.
    ///
    /// Labels are validated when the notification is sent: the placeholder is
    /// limited to 120 characters and the submit label to 40, without controls.
    #[must_use]
    pub fn new(
        action: impl Into<NotificationAction>,
        placeholder: impl Into<String>,
        submit_label: impl Into<String>,
    ) -> Self {
        Self {
            action: action.into(),
            placeholder: placeholder.into(),
            submit_label: submit_label.into(),
        }
    }

    /// Callback or exported action used for an accepted reply.
    #[must_use]
    pub fn action(&self) -> &NotificationAction {
        &self.action
    }

    /// Localized input placeholder.
    #[must_use]
    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    /// Localized submit button text.
    #[must_use]
    pub fn submit_label(&self) -> &str {
        &self.submit_label
    }
}

impl fmt::Debug for NotificationInlineReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotificationInlineReply")
            .field("action", &self.action)
            .field("placeholder_bytes", &self.placeholder.len())
            .field("submit_label_bytes", &self.submit_label.len())
            .finish()
    }
}

/// A validated-at-submission desktop notification.
#[derive(Clone, PartialEq, Eq)]
pub struct DesktopNotification {
    id: NotificationId,
    title: String,
    body: Option<String>,
    markup_body: Option<String>,
    icon: Option<NotificationIcon>,
    priority: NotificationPriority,
    default_action: Option<NotificationAction>,
    buttons: Vec<NotificationButton>,
    inline_reply: Option<NotificationInlineReply>,
    display_hints: Vec<NotificationDisplayHint>,
    category: Option<String>,
}

impl DesktopNotification {
    /// Creates a notification with a stable id and title.
    #[must_use]
    pub fn new(id: NotificationId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            body: None,
            markup_body: None,
            icon: None,
            priority: NotificationPriority::Normal,
            default_action: None,
            buttons: Vec::new(),
            inline_reply: None,
            display_hints: Vec::new(),
            category: None,
        }
    }

    /// Requests a negotiated inline reply; unsupported transports return a typed failure.
    #[must_use]
    pub fn inline_reply(mut self, reply: NotificationInlineReply) -> Self {
        self.inline_reply = Some(reply);
        self
    }

    /// Returns the requested reply, independent of transport support.
    #[must_use]
    pub fn reply_request(&self) -> Option<&NotificationInlineReply> {
        self.inline_reply.as_ref()
    }

    /// Sets a plain-text body.
    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Sets a portal version 2 markup body.
    #[must_use]
    pub fn markup_body(mut self, body: impl Into<String>) -> Self {
        self.markup_body = Some(body.into());
        self
    }

    /// Sets a themed or inline icon.
    #[must_use]
    pub fn icon(mut self, icon: NotificationIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Sets the desktop priority hint.
    #[must_use]
    pub fn priority(mut self, priority: NotificationPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Sets the action used when the notification body is activated.
    #[must_use]
    pub fn default_action(mut self, action: impl Into<NotificationAction>) -> Self {
        self.default_action = Some(action.into());
        self
    }

    /// Adds a labelled action button.
    #[must_use]
    pub fn button(mut self, button: NotificationButton) -> Self {
        self.buttons.push(button);
        self
    }

    /// Replaces advisory display hints, retaining each distinct hint once in input order.
    #[must_use]
    pub fn display_hints(
        mut self,
        hints: impl IntoIterator<Item = NotificationDisplayHint>,
    ) -> Self {
        self.display_hints.clear();
        for hint in hints {
            if !self.display_hints.contains(&hint) {
                self.display_hints.push(hint);
            }
        }
        self
    }

    /// Sets a standard or vendor-prefixed portal category.
    #[must_use]
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// The stable application-local id.
    #[must_use]
    pub fn id(&self) -> &NotificationId {
        &self.id
    }

    /// The title sent to the desktop service.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The plain-text body, when present.
    #[must_use]
    pub fn plain_body(&self) -> Option<&str> {
        self.body.as_deref()
    }

    /// The markup body, when present.
    #[must_use]
    pub fn markup(&self) -> Option<&str> {
        self.markup_body.as_deref()
    }

    /// The icon payload, when present.
    #[must_use]
    pub fn notification_icon(&self) -> Option<&NotificationIcon> {
        self.icon.as_ref()
    }

    /// The requested priority.
    #[must_use]
    pub fn notification_priority(&self) -> NotificationPriority {
        self.priority
    }

    /// The body activation, when present.
    #[must_use]
    pub fn default_activation(&self) -> Option<&NotificationAction> {
        self.default_action.as_ref()
    }

    /// Labelled action buttons.
    #[must_use]
    pub fn action_buttons(&self) -> &[NotificationButton] {
        &self.buttons
    }

    /// Advisory display hints.
    #[must_use]
    pub fn hints(&self) -> &[NotificationDisplayHint] {
        &self.display_hints
    }

    /// The optional category.
    #[must_use]
    pub fn notification_category(&self) -> Option<&str> {
        self.category.as_deref()
    }

    fn validate(&self) -> Result<(), NotificationValidationError> {
        if self.title.is_empty()
            || self.title.len() > MAX_NOTIFICATION_TITLE_BYTES
            || self.title.contains('\0')
        {
            return Err(NotificationValidationError::InvalidTitle);
        }
        for body in [&self.body, &self.markup_body].into_iter().flatten() {
            if body.len() > MAX_NOTIFICATION_BODY_BYTES || body.contains('\0') {
                return Err(NotificationValidationError::InvalidBody);
            }
        }
        if self.body.is_some() && self.markup_body.is_some() {
            return Err(NotificationValidationError::ConflictingBodies);
        }
        if self.buttons.len() + usize::from(self.inline_reply.is_some()) > MAX_NOTIFICATION_BUTTONS
        {
            return Err(NotificationValidationError::TooManyButtons);
        }
        if self
            .display_hints
            .contains(&NotificationDisplayHint::Transient)
            && self.display_hints.contains(&NotificationDisplayHint::Tray)
        {
            return Err(NotificationValidationError::ConflictingDisplayHints);
        }
        if let Some(category) = &self.category
            && (category.is_empty()
                || category.len() > MAX_NOTIFICATION_ID_BYTES
                || !category
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte)))
        {
            return Err(NotificationValidationError::InvalidCategory);
        }
        if let Some(icon) = &self.icon {
            match icon {
                NotificationIcon::Themed(names)
                    if names.is_empty()
                        || names.len() > 8
                        || names.iter().any(|name| {
                            name.is_empty()
                                || name.len() > MAX_NOTIFICATION_ID_BYTES
                                || name.chars().any(char::is_control)
                        }) =>
                {
                    return Err(NotificationValidationError::InvalidIcon);
                }
                NotificationIcon::Bytes(bytes)
                    if bytes.is_empty() || bytes.len() > MAX_NOTIFICATION_ICON_BYTES =>
                {
                    return Err(NotificationValidationError::InvalidIcon);
                }
                _ => {}
            }
        }
        if let Some(reply) = &self.inline_reply {
            for (text, max) in [(&reply.placeholder, 120), (&reply.submit_label, 40)] {
                if text.trim().is_empty()
                    || text.len() > max * 4
                    || text.chars().count() > max
                    || text.chars().any(char::is_control)
                {
                    return Err(NotificationValidationError::InvalidReplyLabel);
                }
            }
        }
        let mut action_ids = HashSet::new();
        for action in self.actions() {
            action
                .target
                .as_ref()
                .map_or(Ok(()), NotificationActionTarget::validate)?;
            if !action_ids.insert(action.wire_name()) {
                return Err(NotificationValidationError::DuplicateActionId);
            }
        }
        for button in &self.buttons {
            if button.label.is_empty()
                || button.label.len() > MAX_NOTIFICATION_TITLE_BYTES
                || button.label.contains('\0')
            {
                return Err(NotificationValidationError::InvalidButtonLabel);
            }
        }
        Ok(())
    }

    fn actions(&self) -> impl Iterator<Item = &NotificationAction> {
        self.default_action
            .iter()
            .chain(self.buttons.iter().map(|button| &button.action))
            .chain(self.inline_reply.iter().map(|reply| &reply.action))
    }

    #[cfg(feature = "portal-dbus")]
    pub(crate) fn has_callback_actions(&self) -> bool {
        self.actions().any(|action| !action.is_exported())
    }

    #[cfg(any(feature = "portal-dbus", feature = "notification-freedesktop"))]
    pub(crate) fn has_exported_actions(&self) -> bool {
        self.actions().any(NotificationAction::is_exported)
    }

    fn action_map(&self) -> HashMap<NotificationActionId, Option<NotificationActionTarget>> {
        self.actions()
            .filter(|action| !action.is_exported())
            .map(|action| (action.id.clone(), action.target.clone()))
            .collect()
    }
}

impl fmt::Debug for DesktopNotification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DesktopNotification")
            .field("id", &self.id)
            .field("title_bytes", &self.title.len())
            .field("body_bytes", &self.body.as_ref().map(String::len))
            .field(
                "markup_body_bytes",
                &self.markup_body.as_ref().map(String::len),
            )
            .field("icon", &self.icon)
            .field("priority", &self.priority)
            .field("default_action", &self.default_action)
            .field("button_count", &self.buttons.len())
            .field("inline_reply", &self.inline_reply)
            .field("display_hints", &self.display_hints)
            .field("category", &self.category)
            .finish()
    }
}

/// A payload validation failure detected before any desktop request is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationValidationError {
    /// Invalid stable notification id.
    InvalidNotificationId,
    /// Invalid reducer action name.
    InvalidActionId,
    /// Empty, oversized, or NUL-containing title.
    InvalidTitle,
    /// Oversized or NUL-containing body.
    InvalidBody,
    /// Plain and markup bodies were both supplied.
    ConflictingBodies,
    /// More than [`MAX_NOTIFICATION_BUTTONS`] were supplied.
    TooManyButtons,
    /// Transient and tray hints were combined.
    ConflictingDisplayHints,
    /// Empty, oversized, or malformed button label.
    InvalidButtonLabel,
    /// Empty, oversized, or control-containing inline reply label.
    InvalidReplyLabel,
    /// A notification repeats one action id.
    DuplicateActionId,
    /// Empty, oversized, or malformed icon data.
    InvalidIcon,
    /// Empty, oversized, or malformed category.
    InvalidCategory,
    /// Empty, oversized, or malformed button purpose.
    InvalidPurpose,
    /// A text action value contains a NUL byte.
    InvalidActionValue,
    /// An action value exceeds [`MAX_NOTIFICATION_ACTION_VALUE_BYTES`].
    ActionValueTooLarge,
}

impl fmt::Display for NotificationValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::InvalidNotificationId => "invalid notification id",
            Self::InvalidActionId => "invalid notification action id",
            Self::InvalidTitle => "invalid notification title",
            Self::InvalidBody => "invalid notification body",
            Self::ConflictingBodies => "plain and markup bodies are mutually exclusive",
            Self::TooManyButtons => "too many notification buttons",
            Self::ConflictingDisplayHints => "transient and tray hints conflict",
            Self::InvalidButtonLabel => "invalid notification button label",
            Self::InvalidReplyLabel => "invalid inline reply label",
            Self::DuplicateActionId => "duplicate notification action id",
            Self::InvalidIcon => "invalid notification icon",
            Self::InvalidCategory => "invalid notification category",
            Self::InvalidPurpose => "invalid notification button purpose",
            Self::InvalidActionValue => "invalid notification action value",
            Self::ActionValueTooLarge => "notification action value is too large",
        };
        f.write_str(text)
    }
}

impl std::error::Error for NotificationValidationError {}

/// A notification field or semantic option unavailable on the selected service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsupportedNotificationFeature {
    /// The selected notification interface itself is absent.
    Notifications,
    /// The backend could not subscribe to reducer action callbacks.
    ActionDelivery,
    /// The service or selected transport cannot deliver inline replies.
    InlineReply,
    /// Exported actions require an application-bound portal connection.
    ExportedAction,
    /// The direct transport cannot represent the supplied encoded icon.
    EncodedIcon,
    /// Markup requires portal version 2 or direct-service `body-markup` support.
    MarkupBody,
    /// Display hints require portal version 2; the direct backend accepts transient and persistent.
    DisplayHints,
    /// The service did not advertise this category.
    Category(String),
    /// The service did not advertise this button purpose.
    ButtonPurpose(String),
}

/// A nonblocking transport submission failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationBackendError {
    /// Service readiness has not settled; retry after the next wake.
    Initialising,
    /// No compatible notification service is available.
    Unavailable,
    /// The service lacks a requested field or interface.
    Unsupported(UnsupportedNotificationFeature),
    /// The bounded command queue has no capacity.
    ResourceExhausted,
    /// The bus or portal refused the call.
    Denied,
    /// Another transport or protocol failure occurred.
    Failed,
}

impl fmt::Display for NotificationBackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initialising => f.write_str("desktop notifications are still initialising"),
            Self::Unavailable => f.write_str("desktop notifications are unavailable"),
            Self::Unsupported(feature) => {
                write!(
                    f,
                    "desktop notification feature is unsupported: {feature:?}"
                )
            }
            Self::ResourceExhausted => f.write_str("notification command queue is full"),
            Self::Denied => f.write_str("desktop notification service denied the request"),
            Self::Failed => f.write_str("desktop notification request failed"),
        }
    }
}

impl std::error::Error for NotificationBackendError {}

/// Opaque id assigned to one backend command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NotificationOperationId(u64);

impl NotificationOperationId {
    /// Creates an operation id from a transport-owned value.
    #[must_use]
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The transport-owned numeric value.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Transport command used by desktop and deterministic mock backends.
#[derive(Clone, PartialEq, Eq)]
pub enum NotificationBackendCommand {
    /// Add or replace a notification.
    Send(Box<DesktopNotification>),
    /// Remove a notification by application-local id.
    Withdraw(NotificationId),
}

impl fmt::Debug for NotificationBackendCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Send(notification) => f.debug_tuple("Send").field(notification).finish(),
            Self::Withdraw(id) => f.debug_tuple("Withdraw").field(id).finish(),
        }
    }
}

/// Completion reported by a notification transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationBackendOutcome {
    /// The service accepted a send; presentation remains subject to desktop policy.
    Sent,
    /// The service accepted a withdrawal.
    Withdrawn,
    /// The owning portal incarnation vanished before completion.
    Unavailable,
    /// The service rejected an unsupported field.
    Unsupported(UnsupportedNotificationFeature),
    /// The service denied the operation.
    Denied,
    /// The call failed or returned a malformed response.
    Failed,
}

/// A raw action callback accepted from the current service owner.
#[derive(PartialEq, Eq)]
pub struct NotificationBackendAction {
    /// Notification id supplied by the service.
    pub notification_id: String,
    /// Action name supplied by the service.
    pub action_id: String,
    /// Target echoed by the service, if understood.
    pub target: Option<NotificationActionTarget>,
    /// Native or protocol-specific activation context supplied by the desktop.
    ///
    /// Use [`crate::activation::InboundActivationToken::native`] for the
    /// freedesktop `ActivationToken` signal, which does not identify its protocol.
    pub activation_token: Option<crate::activation::InboundActivationToken>,
    /// User response supplied by a purpose such as inline reply.
    pub response: Option<String>,
    /// Whether an unrecognized parameter was present.
    pub malformed: bool,
    /// Service owner generation that emitted the callback.
    pub owner_generation: u64,
    /// Monotonic instant when the transport accepted the callback.
    pub received_at: Instant,
}

impl fmt::Debug for NotificationBackendAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotificationBackendAction")
            .field("notification_id", &self.notification_id)
            .field("action_id", &self.action_id)
            .field("has_target", &self.target.is_some())
            .field("has_activation_token", &self.activation_token.is_some())
            .field("has_response", &self.response.is_some())
            .field("malformed", &self.malformed)
            .field("owner_generation", &self.owner_generation)
            .field("received_at", &self.received_at)
            .finish()
    }
}

/// One event produced by a notification transport.
#[derive(Debug, PartialEq, Eq)]
pub enum NotificationBackendEvent {
    /// A submitted command reached a terminal state.
    Operation {
        /// Command id.
        operation: NotificationOperationId,
        /// Service generation used for the call.
        owner_generation: u64,
        /// Terminal result.
        outcome: NotificationBackendOutcome,
    },
    /// The service owner changed or disappeared.
    OwnerChanged {
        /// New monotonic owner generation.
        generation: u64,
        /// Whether a replacement owner is currently ready.
        available: bool,
    },
    /// The backend's action signal subscription terminated.
    ActionDeliveryUnavailable,
    /// A desktop action was invoked.
    Action(NotificationBackendAction),
    /// The desktop closed a shown notification (it expired, the user
    /// dismissed it, or it was withdrawn).
    Closed {
        /// The application-local id the notification was sent with.
        notification_id: NotificationId,
        /// Service generation that reported the closure.
        owner_generation: u64,
        /// Why it closed.
        reason: NotificationCloseReason,
    },
}

/// Why the desktop closed a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationCloseReason {
    /// It expired.
    Expired,
    /// The user dismissed it.
    Dismissed,
    /// The application withdrew it.
    Withdrawn,
    /// The service gave no reason, or one this client does not know.
    Other,
}

impl NotificationCloseReason {
    /// The reason for a freedesktop `NotificationClosed` reason code.
    #[must_use]
    pub const fn from_freedesktop(code: u32) -> Self {
        match code {
            1 => Self::Expired,
            2 => Self::Dismissed,
            3 => Self::Withdrawn,
            _ => Self::Other,
        }
    }
}

/// Bounded action-delivery diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NotificationBackendStats {
    /// Raw callbacks currently retained by the transport.
    pub pending_actions: usize,
    /// Hard action queue capacity.
    pub action_capacity: usize,
    /// Callbacks dropped because the action queue was full.
    pub dropped_actions: u64,
    /// Callbacks rejected before enqueue because the encoded wire payload or a
    /// typed field exceeded its reviewed cap, or the sender was not the current
    /// service owner.
    pub rejected_actions: u64,
}

/// Nonblocking backend contract for desktop notifications.
pub trait NotificationBackend {
    /// The application identity fixed when this transport connection was created, if any.
    fn notification_application_id(&self) -> Option<&AppId> {
        None
    }

    /// Submits a validated command without waiting on D-Bus.
    ///
    /// Implementations must honor [`NotificationAction::is_exported`] or
    /// report [`UnsupportedNotificationFeature::ExportedAction`]; exported
    /// actions must never be reinterpreted as reducer callbacks.
    ///
    /// # Errors
    ///
    /// A [`NotificationBackendError`] when the backend is initialising or cannot accept the
    /// command.
    fn submit_notification(
        &mut self,
        command: NotificationBackendCommand,
    ) -> Result<NotificationOperationId, NotificationBackendError>;

    /// Takes the next completion, owner, or action event.
    fn poll_notification_event(&mut self) -> Option<NotificationBackendEvent>;

    /// Event edge used to wake an idle iced runtime.
    fn notification_completion_signal(&self) -> Option<CompletionSignal> {
        None
    }

    /// Current bounded queue pressure.
    fn notification_stats(&self) -> NotificationBackendStats {
        NotificationBackendStats {
            action_capacity: NOTIFICATION_ACTION_QUEUE_CAPACITY,
            ..NotificationBackendStats::default()
        }
    }
}

/// Monotonic local generation assigned whenever an id is sent or replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NotificationGeneration(u64);

impl NotificationGeneration {
    /// The application-local generation value.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Whether an accepted send creates or replaces an application-local id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationSendKind {
    /// First currently tracked use of this id.
    New,
    /// Replacement of the currently tracked id.
    Replacement,
}

/// Accepted nonblocking command receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationReceipt {
    /// Backend operation id.
    pub operation: NotificationOperationId,
    /// Application-local notification id.
    pub notification_id: NotificationId,
    /// Generation affected by this command.
    pub generation: NotificationGeneration,
    /// Command kind.
    pub kind: NotificationReceiptKind,
}

/// Kind of an accepted command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationReceiptKind {
    /// AddNotification, including replacement semantics.
    Send(NotificationSendKind),
    /// RemoveNotification.
    Withdraw,
}

/// A client-side submission refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationError {
    /// The client identity differs from the transport's configured application identity.
    ApplicationIdentityMismatch,
    /// The client has begun shutdown and accepts no new commands.
    ShuttingDown,
    /// Payload validation failed before backend submission.
    InvalidPayload(NotificationValidationError),
    /// The requested id is not currently tracked.
    UnknownId,
    /// The supplied generation no longer names the current notification.
    StaleGeneration,
    /// The active-notification bound was reached.
    Capacity {
        /// Configured maximum active identities.
        capacity: usize,
    },
    /// Too many accepted commands still await terminal events.
    PendingCapacity {
        /// Configured maximum pending operations.
        capacity: usize,
    },
    /// Backend submission failed.
    Backend(NotificationBackendError),
    /// The portal is still starting. The client holds this notification, the
    /// latest per id, and sends it once the portal is ready; see
    /// [`DesktopNotificationClient::send`].
    Deferred,
}

impl fmt::Display for NotificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationIdentityMismatch => {
                f.write_str("notification client and transport application identities differ")
            }
            Self::ShuttingDown => f.write_str("notification client is shutting down"),
            Self::InvalidPayload(error) => write!(f, "invalid desktop notification: {error}"),
            Self::UnknownId => f.write_str("notification id is not tracked"),
            Self::StaleGeneration => f.write_str("notification generation is stale"),
            Self::Capacity { capacity } => {
                write!(f, "active notification limit reached ({capacity})")
            }
            Self::PendingCapacity { capacity } => {
                write!(f, "pending notification limit reached ({capacity})")
            }
            Self::Backend(error) => error.fmt(f),
            Self::Deferred => f.write_str("notification held until the portal is ready"),
        }
    }
}

impl std::error::Error for NotificationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidPayload(error) => Some(error),
            Self::Backend(error) => Some(error),
            _ => None,
        }
    }
}

/// Reducer-visible terminal command outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationOperationOutcome {
    /// A new notification was accepted by the portal.
    Sent,
    /// An existing id was updated by the portal.
    Replaced,
    /// The notification was withdrawn.
    Withdrawn,
    /// A newer local command superseded this completion.
    Superseded,
    /// The owning portal incarnation disappeared.
    Unavailable,
    /// A requested field is not supported.
    Unsupported(UnsupportedNotificationFeature),
    /// The desktop denied the operation.
    Denied,
    /// The operation failed.
    Failed,
}

/// Platform activation context retained from a portal callback.
#[derive(PartialEq, Eq)]
pub enum NotificationActivationContext {
    /// XDG activation token supplied by the desktop.
    ///
    /// The validated callback owns this value until
    /// [`crate::activation::ActivationBroker::admit_notification_invocation`]
    /// moves it into one expiring activation context.
    XdgActivationToken(crate::activation::InboundActivationToken),
    /// Protocol-specific fields, or an untyped native token, retained without relabeling.
    PlatformData(crate::activation::InboundActivationToken),
    /// The service supplied no activation token.
    Unavailable,
}

impl fmt::Debug for NotificationActivationContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::XdgActivationToken(_) => f.write_str("XdgActivationToken(<redacted>)"),
            Self::PlatformData(_) => f.write_str("PlatformData(<redacted>)"),
            Self::Unavailable => f.write_str("Unavailable"),
        }
    }
}

/// One action routed to the notification-owning reducer.
#[derive(PartialEq, Eq)]
pub struct NotificationActionInvocation {
    /// Validated application identity supplied when the client was built.
    pub application_id: AppId,
    /// Stable notification id.
    pub notification_id: NotificationId,
    /// Local generation whose action was invoked.
    pub generation: NotificationGeneration,
    /// Registered action id.
    pub action_id: NotificationActionId,
    /// Registered and wire-verified target.
    pub target: Option<NotificationActionTarget>,
    /// Platform activation data, if supplied.
    pub activation_context: NotificationActivationContext,
    /// Monotonic instant when the transport accepted the callback.
    pub received_at: Instant,
    /// Optional bounded user response, such as inline reply text.
    pub response: Option<String>,
}

impl fmt::Debug for NotificationActionInvocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotificationActionInvocation")
            .field("application_id", &self.application_id)
            .field("notification_id", &self.notification_id)
            .field("generation", &self.generation)
            .field("action_id", &self.action_id)
            .field("has_target", &self.target.is_some())
            .field("activation_context", &self.activation_context)
            .field("received_at", &self.received_at)
            .field("has_response", &self.response.is_some())
            .finish()
    }
}

/// Why a raw callback was not delivered to the reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IgnoredNotificationAction {
    /// No live notification has this id.
    UnknownNotification,
    /// The callback came from an older portal owner.
    StaleOwner,
    /// The current notification did not register this action.
    UnknownAction,
    /// The callback target did not match the registered target.
    TargetMismatch,
    /// This action already reached the reducer for this notification generation.
    Duplicate,
    /// A returned activation token or response exceeded the retained bound.
    OversizedContext,
    /// The service supplied an unrecognized action parameter.
    MalformedPayload,
}

/// One event for the owning application's reducer.
#[derive(Debug, PartialEq, Eq)]
pub enum NotificationEvent {
    /// A command completed.
    Operation {
        /// Original receipt.
        receipt: NotificationReceipt,
        /// Terminal result.
        outcome: NotificationOperationOutcome,
    },
    /// The backend can still send notifications but cannot receive actions.
    ActionDeliveryUnavailable,
    /// A user action accepted exactly once.
    Action(NotificationActionInvocation),
    /// A malformed, stale, duplicate, or unowned action was rejected.
    IgnoredAction {
        /// Untrusted notification id, retained only when it fits the id bound.
        notification_id: Option<NotificationId>,
        /// Rejection class.
        reason: IgnoredNotificationAction,
    },
    /// The desktop closed a notification. Its registration is released, so
    /// the id counts no more toward the active limit and its actions stop
    /// routing.
    Closed {
        /// The closed notification.
        notification_id: NotificationId,
        /// The generation that was showing.
        generation: NotificationGeneration,
        /// Why it closed.
        reason: NotificationCloseReason,
    },
    /// The service owner changed; notifications from the old incarnation are no longer routable.
    ServiceChanged {
        /// New monotonic owner generation.
        generation: u64,
        /// Whether a replacement service is ready.
        available: bool,
    },
}

#[derive(Clone)]
struct ActiveNotification {
    generation: NotificationGeneration,
    owner_generation: u64,
    actions: HashMap<NotificationActionId, Option<NotificationActionTarget>>,
}

struct PendingNotification {
    receipt: NotificationReceipt,
    previous_generation: Option<NotificationGeneration>,
    actions: HashMap<NotificationActionId, Option<NotificationActionTarget>>,
}

/// Policy applied when a notification client begins shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationShutdown {
    /// Leave posted notifications to the desktop's persistence policy.
    LeavePosted,
    /// Submit a withdrawal for every tracked application-local id.
    Withdraw,
}

/// Application-owned desktop-notification client.
///
/// Dropping the client leaves already-sent notifications to the desktop, as the
/// portal specification permits. Call [`Self::begin_shutdown`] with
/// [`NotificationShutdown::Withdraw`] when the application wants them removed.
pub struct DesktopNotificationClient<B> {
    application_id: AppId,
    backend: B,
    shutting_down: bool,
    owner_generation: Option<u64>,
    owner_available: bool,
    next_generation: u64,
    pending: HashMap<NotificationOperationId, PendingNotification>,
    current: HashMap<NotificationId, NotificationGeneration>,
    active: HashMap<NotificationId, ActiveNotification>,
    delivered: HashSet<(NotificationId, NotificationGeneration, NotificationActionId)>,
    // Sends refused while the portal was starting, the latest per id, in order.
    deferred: Vec<DesktopNotification>,
}

impl<B: NotificationBackend> DesktopNotificationClient<B> {
    /// Creates a client bound to an existing validated application identity.
    ///
    /// The identity scopes reducer events and must match an application-bound
    /// transport. For host portal identity and exported actions, construct the
    /// backend with `DbusBackend::for_application` before creating this client.
    /// Sandboxed callers use their portal-assigned identity; direct backends
    /// supply the desktop-entry hint from their configured application identity.
    #[must_use]
    pub fn new(application_id: AppId, backend: B) -> Self {
        Self {
            application_id,
            backend,
            shutting_down: false,
            owner_generation: None,
            owner_available: false,
            next_generation: 0,
            pending: HashMap::new(),
            current: HashMap::new(),
            active: HashMap::new(),
            delivered: HashSet::new(),
            deferred: Vec::new(),
        }
    }

    /// The application identity used to scope reducer delivery.
    #[must_use]
    pub fn application_id(&self) -> &AppId {
        &self.application_id
    }

    /// Accesses the backend for diagnostics or deterministic test control.
    #[must_use]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Mutably accesses the backend for deterministic test control.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Submits a new notification or replaces the current notification with the same id.
    ///
    /// Payload text is validated before it crosses the backend boundary. A
    /// successful receipt means queued, not displayed; the portal does not
    /// report whether desktop policy showed it.
    ///
    /// A backend still starting refuses with
    /// [`NotificationBackendError::Initialising`]. The client then holds the
    /// notification and returns [`NotificationError::Deferred`]. It submits
    /// the held copy when [`poll_event`](Self::poll_event) sees the portal
    /// become ready, which the [`wake_subscription`](Self::wake_subscription)
    /// announces. So an app's first notification after launch is not lost. A
    /// later send or withdrawal of the same id replaces or drops the held copy.
    ///
    /// # Errors
    ///
    /// A [`NotificationError`]: `Deferred` while the backend initialises, and others when it
    /// refuses the notification.
    pub fn send(
        &mut self,
        notification: DesktopNotification,
    ) -> Result<NotificationReceipt, NotificationError> {
        let result = self.submit_send(notification.clone());
        let id = notification.id();
        self.deferred.retain(|held| held.id() != id);
        if let Err(NotificationError::Backend(NotificationBackendError::Initialising)) = result
            && self.deferred.len() < MAX_ACTIVE_NOTIFICATIONS
        {
            self.deferred.push(notification);
            return Err(NotificationError::Deferred);
        }
        result
    }

    // Submit every held send, keeping any the backend still defers.
    fn submit_deferred(&mut self) {
        for notification in std::mem::take(&mut self.deferred) {
            let _ = self.send(notification);
        }
    }

    fn submit_send(
        &mut self,
        notification: DesktopNotification,
    ) -> Result<NotificationReceipt, NotificationError> {
        if self
            .backend
            .notification_application_id()
            .is_some_and(|id| id != &self.application_id)
        {
            return Err(NotificationError::ApplicationIdentityMismatch);
        }
        if self.shutting_down {
            return Err(NotificationError::ShuttingDown);
        }
        if self.pending.len() >= MAX_PENDING_NOTIFICATION_OPERATIONS {
            return Err(NotificationError::PendingCapacity {
                capacity: MAX_PENDING_NOTIFICATION_OPERATIONS,
            });
        }
        notification
            .validate()
            .map_err(NotificationError::InvalidPayload)?;
        let id = notification.id.clone();
        let previous_generation = self.current.get(&id).copied();
        let replacing = previous_generation.is_some();
        if !replacing && self.current.len() >= MAX_ACTIVE_NOTIFICATIONS {
            return Err(NotificationError::Capacity {
                capacity: MAX_ACTIVE_NOTIFICATIONS,
            });
        }
        let generation = self.allocate_generation();
        let actions = notification.action_map();
        let operation = self
            .backend
            .submit_notification(NotificationBackendCommand::Send(Box::new(notification)))
            .map_err(NotificationError::Backend)?;
        let receipt = NotificationReceipt {
            operation,
            notification_id: id.clone(),
            generation,
            kind: NotificationReceiptKind::Send(if replacing {
                NotificationSendKind::Replacement
            } else {
                NotificationSendKind::New
            }),
        };
        self.current.insert(id, generation);
        self.pending.insert(
            operation,
            PendingNotification {
                receipt: receipt.clone(),
                previous_generation,
                actions,
            },
        );
        Ok(receipt)
    }

    /// Replaces only when `expected` is still the current local generation.
    ///
    /// # Errors
    ///
    /// [`NotificationError::UnknownId`] for an id not shown, `StaleGeneration` for a superseded
    /// one, and backend refusals.
    pub fn replace(
        &mut self,
        expected: NotificationGeneration,
        notification: DesktopNotification,
    ) -> Result<NotificationReceipt, NotificationError> {
        match self.current.get(notification.id()) {
            None => return Err(NotificationError::UnknownId),
            Some(current) if *current != expected => {
                return Err(NotificationError::StaleGeneration);
            }
            Some(_) => {}
        }
        self.send(notification)
    }

    /// Withdraws the current generation of `id`, and drops a held send of it.
    ///
    /// # Errors
    ///
    /// [`NotificationError::ShuttingDown`] during shutdown, and backend refusals.
    pub fn withdraw(
        &mut self,
        id: &NotificationId,
    ) -> Result<NotificationReceipt, NotificationError> {
        self.deferred.retain(|held| held.id() != id);
        if self.shutting_down {
            return Err(NotificationError::ShuttingDown);
        }
        self.submit_withdraw(id)
    }

    fn submit_withdraw(
        &mut self,
        id: &NotificationId,
    ) -> Result<NotificationReceipt, NotificationError> {
        if self.pending.len() >= MAX_PENDING_NOTIFICATION_OPERATIONS {
            return Err(NotificationError::PendingCapacity {
                capacity: MAX_PENDING_NOTIFICATION_OPERATIONS,
            });
        }
        let Some(generation) = self.current.get(id).copied() else {
            return Err(NotificationError::UnknownId);
        };
        let operation = self
            .backend
            .submit_notification(NotificationBackendCommand::Withdraw(id.clone()))
            .map_err(NotificationError::Backend)?;
        let receipt = NotificationReceipt {
            operation,
            notification_id: id.clone(),
            generation,
            kind: NotificationReceiptKind::Withdraw,
        };
        self.pending.insert(
            operation,
            PendingNotification {
                receipt: receipt.clone(),
                previous_generation: Some(generation),
                actions: HashMap::new(),
            },
        );
        Ok(receipt)
    }

    /// Withdraws only when `expected` is still current.
    ///
    /// # Errors
    ///
    /// [`NotificationError::UnknownId`] for an id not shown, and `StaleGeneration` when `expected`
    /// was superseded.
    pub fn withdraw_generation(
        &mut self,
        id: &NotificationId,
        expected: NotificationGeneration,
    ) -> Result<NotificationReceipt, NotificationError> {
        match self.current.get(id) {
            None => Err(NotificationError::UnknownId),
            Some(current) if *current != expected => Err(NotificationError::StaleGeneration),
            Some(_) => self.withdraw(id),
        }
    }

    /// Begins shutdown and optionally submits bounded withdrawals for every tracked id.
    ///
    /// [`NotificationShutdown::LeavePosted`] follows portal persistence semantics:
    /// dropping the process does not withdraw desktop notifications. With
    /// [`NotificationShutdown::Withdraw`], continue draining returned receipts
    /// until terminal events arrive before dropping the client. A partial
    /// submission keeps the client open so the caller can drain and retry.
    pub fn begin_shutdown(
        &mut self,
        policy: NotificationShutdown,
    ) -> Vec<Result<NotificationReceipt, NotificationError>> {
        if self.shutting_down {
            return Vec::new();
        }
        self.deferred.clear();
        let ids = if policy == NotificationShutdown::Withdraw {
            self.current.keys().cloned().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let receipts = ids
            .iter()
            .map(|id| self.submit_withdraw(id))
            .collect::<Vec<_>>();
        self.shutting_down = receipts.iter().all(Result::is_ok);
        receipts
    }

    /// Whether shutdown has begun.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down
    }

    /// Takes the next reducer event without blocking.
    #[must_use]
    pub fn poll_event(&mut self) -> Option<NotificationEvent> {
        // A closure that changes nothing (an old owner's, or one racing a
        // replacement) yields no event; keep taking until one does.
        loop {
            let event = self.backend.poll_notification_event()?;
            if let Some(event) = self.reduce(event) {
                return Some(event);
            }
        }
    }

    fn reduce(&mut self, event: NotificationBackendEvent) -> Option<NotificationEvent> {
        match event {
            NotificationBackendEvent::Operation {
                operation,
                owner_generation,
                outcome,
            } => self.finish_operation(operation, owner_generation, outcome),
            NotificationBackendEvent::OwnerChanged {
                generation,
                available,
            } => {
                self.owner_generation = Some(generation);
                self.owner_available = available;
                self.active.clear();
                self.delivered.clear();
                if available {
                    self.submit_deferred();
                }
                Some(NotificationEvent::ServiceChanged {
                    generation,
                    available,
                })
            }
            NotificationBackendEvent::ActionDeliveryUnavailable => {
                self.active.clear();
                self.delivered.clear();
                Some(NotificationEvent::ActionDeliveryUnavailable)
            }
            NotificationBackendEvent::Action(action) => Some(self.route_action(action)),
            NotificationBackendEvent::Closed {
                notification_id,
                owner_generation,
                reason,
            } => self.retire_closed(notification_id, owner_generation, reason),
        }
    }

    /// Releases a notification the desktop closed. A closure from an older
    /// service owner, or for an id with a send or withdrawal still in flight
    /// (whose newer generation the closure does not describe), changes
    /// nothing.
    fn retire_closed(
        &mut self,
        notification_id: NotificationId,
        owner_generation: u64,
        reason: NotificationCloseReason,
    ) -> Option<NotificationEvent> {
        // The registration names the owner that showed it; without one, the
        // client's current owner.
        let shown_by = self
            .active
            .get(&notification_id)
            .map(|active| active.owner_generation)
            .or(self.owner_generation);
        if shown_by.is_some_and(|generation| generation != owner_generation) {
            return None;
        }
        if self
            .pending
            .values()
            .any(|pending| pending.receipt.notification_id == notification_id)
        {
            return None;
        }
        let generation = self.current.remove(&notification_id)?;
        self.active.remove(&notification_id);
        self.delivered.retain(|(id, ..)| *id != notification_id);
        Some(NotificationEvent::Closed {
            notification_id,
            generation,
            reason,
        })
    }

    /// Drains at most `limit` reducer events.
    #[must_use]
    pub fn drain_events(&mut self, limit: usize) -> Vec<NotificationEvent> {
        let mut events = Vec::with_capacity(limit.min(NOTIFICATION_ACTION_QUEUE_CAPACITY));
        while events.len() < limit {
            let Some(event) = self.poll_event() else {
                break;
            };
            events.push(event);
        }
        events
    }

    /// Returns the current generation for an id, including an accepted send still in flight.
    #[must_use]
    pub fn current_generation(&self, id: &NotificationId) -> Option<NotificationGeneration> {
        self.current.get(id).copied()
    }

    /// Returns backend action-queue pressure and overflow totals.
    #[must_use]
    pub fn backend_stats(&self) -> NotificationBackendStats {
        self.backend.notification_stats()
    }

    /// Builds an event-driven iced subscription for backend completions and actions.
    ///
    /// Events accepted before the first poll produce a retained wake. Returns no
    /// subscription when the backend supplies no wake signal.
    pub fn wake_subscription(&self) -> Subscription<NotificationWake> {
        let Some(signal) = self.backend.notification_completion_signal() else {
            return Subscription::none();
        };
        Subscription::run_with(NotificationWakeSubscription { signal }, |data| {
            let signal = data.signal.clone();
            iced_futures::futures::stream::unfold((signal, 0), |(signal, seen)| async move {
                let generation = signal.wait_after(seen).await;
                Some((NotificationWake { generation }, (signal, generation)))
            })
        })
    }

    fn allocate_generation(&mut self) -> NotificationGeneration {
        self.next_generation = self.next_generation.saturating_add(1);
        NotificationGeneration(self.next_generation)
    }

    fn finish_operation(
        &mut self,
        operation: NotificationOperationId,
        owner_generation: u64,
        backend_outcome: NotificationBackendOutcome,
    ) -> Option<NotificationEvent> {
        let pending = self.pending.remove(&operation)?;
        let receipt = pending.receipt;
        let stale_owner = self
            .owner_generation
            .is_some_and(|current| current != owner_generation || !self.owner_available);
        let backend_outcome = if stale_owner
            && matches!(
                backend_outcome,
                NotificationBackendOutcome::Sent | NotificationBackendOutcome::Withdrawn
            ) {
            NotificationBackendOutcome::Unavailable
        } else {
            if self.owner_generation.is_none() {
                self.owner_generation = Some(owner_generation);
                self.owner_available = true;
            }
            backend_outcome
        };
        let is_current =
            self.current.get(&receipt.notification_id).copied() == Some(receipt.generation);
        let outcome = match backend_outcome {
            NotificationBackendOutcome::Sent if !is_current => {
                if self
                    .active
                    .get(&receipt.notification_id)
                    .is_none_or(|active| active.generation < receipt.generation)
                {
                    self.active.insert(
                        receipt.notification_id.clone(),
                        ActiveNotification {
                            generation: receipt.generation,
                            owner_generation,
                            actions: pending.actions,
                        },
                    );
                }
                NotificationOperationOutcome::Superseded
            }
            NotificationBackendOutcome::Sent => {
                self.delivered
                    .retain(|(id, _, _)| id != &receipt.notification_id);
                self.active.insert(
                    receipt.notification_id.clone(),
                    ActiveNotification {
                        generation: receipt.generation,
                        owner_generation,
                        actions: pending.actions,
                    },
                );
                match receipt.kind {
                    NotificationReceiptKind::Send(NotificationSendKind::New) => {
                        NotificationOperationOutcome::Sent
                    }
                    NotificationReceiptKind::Send(NotificationSendKind::Replacement) => {
                        NotificationOperationOutcome::Replaced
                    }
                    NotificationReceiptKind::Withdraw => NotificationOperationOutcome::Failed,
                }
            }
            NotificationBackendOutcome::Withdrawn if !is_current => {
                NotificationOperationOutcome::Superseded
            }
            NotificationBackendOutcome::Withdrawn => {
                self.current.remove(&receipt.notification_id);
                self.active.remove(&receipt.notification_id);
                self.delivered
                    .retain(|(id, _, _)| id != &receipt.notification_id);
                NotificationOperationOutcome::Withdrawn
            }
            NotificationBackendOutcome::Unavailable => {
                self.rollback_failed_send(&receipt, pending.previous_generation);
                NotificationOperationOutcome::Unavailable
            }
            NotificationBackendOutcome::Unsupported(feature) => {
                self.rollback_failed_send(&receipt, pending.previous_generation);
                NotificationOperationOutcome::Unsupported(feature)
            }
            NotificationBackendOutcome::Denied => {
                self.rollback_failed_send(&receipt, pending.previous_generation);
                NotificationOperationOutcome::Denied
            }
            NotificationBackendOutcome::Failed => {
                self.rollback_failed_send(&receipt, pending.previous_generation);
                NotificationOperationOutcome::Failed
            }
        };
        Some(NotificationEvent::Operation { receipt, outcome })
    }

    fn rollback_failed_send(
        &mut self,
        receipt: &NotificationReceipt,
        previous_generation: Option<NotificationGeneration>,
    ) {
        if !matches!(receipt.kind, NotificationReceiptKind::Send(_))
            || self.current.get(&receipt.notification_id).copied() != Some(receipt.generation)
        {
            return;
        }
        match previous_generation {
            Some(generation) => {
                self.current
                    .insert(receipt.notification_id.clone(), generation);
            }
            None => {
                self.current.remove(&receipt.notification_id);
            }
        }
    }

    fn route_action(&mut self, raw: NotificationBackendAction) -> NotificationEvent {
        let parsed_id = NotificationId::new(raw.notification_id).ok();
        let Some(notification_id) = parsed_id else {
            return NotificationEvent::IgnoredAction {
                notification_id: None,
                reason: IgnoredNotificationAction::UnknownNotification,
            };
        };
        if self
            .owner_generation
            .is_some_and(|generation| generation != raw.owner_generation || !self.owner_available)
        {
            return NotificationEvent::IgnoredAction {
                notification_id: Some(notification_id),
                reason: IgnoredNotificationAction::StaleOwner,
            };
        }
        let Some(active) = self.active.get(&notification_id).cloned() else {
            return NotificationEvent::IgnoredAction {
                notification_id: Some(notification_id),
                reason: IgnoredNotificationAction::UnknownNotification,
            };
        };
        if active.owner_generation != raw.owner_generation {
            return NotificationEvent::IgnoredAction {
                notification_id: Some(notification_id),
                reason: IgnoredNotificationAction::StaleOwner,
            };
        }
        let Ok(action_id) = NotificationActionId::new(raw.action_id) else {
            return NotificationEvent::IgnoredAction {
                notification_id: Some(notification_id),
                reason: IgnoredNotificationAction::UnknownAction,
            };
        };
        let Some(expected_target) = active.actions.get(&action_id) else {
            return NotificationEvent::IgnoredAction {
                notification_id: Some(notification_id),
                reason: IgnoredNotificationAction::UnknownAction,
            };
        };
        if expected_target != &raw.target {
            return NotificationEvent::IgnoredAction {
                notification_id: Some(notification_id),
                reason: IgnoredNotificationAction::TargetMismatch,
            };
        }
        if raw.malformed {
            return NotificationEvent::IgnoredAction {
                notification_id: Some(notification_id),
                reason: IgnoredNotificationAction::MalformedPayload,
            };
        }
        if raw
            .activation_token
            .as_ref()
            .is_some_and(|token| token.byte_len() > MAX_NOTIFICATION_ACTION_VALUE_BYTES)
            || raw
                .response
                .as_ref()
                .is_some_and(|response| response.len() > MAX_NOTIFICATION_ACTION_VALUE_BYTES)
        {
            return NotificationEvent::IgnoredAction {
                notification_id: Some(notification_id),
                reason: IgnoredNotificationAction::OversizedContext,
            };
        }
        let delivery_key = (
            notification_id.clone(),
            active.generation,
            action_id.clone(),
        );
        if !self.delivered.insert(delivery_key) {
            return NotificationEvent::IgnoredAction {
                notification_id: Some(notification_id),
                reason: IgnoredNotificationAction::Duplicate,
            };
        }
        NotificationEvent::Action(NotificationActionInvocation {
            application_id: self.application_id.clone(),
            notification_id,
            generation: active.generation,
            action_id,
            target: raw.target,
            activation_context: raw.activation_token.map_or(
                NotificationActivationContext::Unavailable,
                |token| {
                    if token.xdg_activation_token().is_some()
                        && token.desktop_startup_id().is_none()
                    {
                        NotificationActivationContext::XdgActivationToken(token)
                    } else {
                        NotificationActivationContext::PlatformData(token)
                    }
                },
            ),
            received_at: raw.received_at,
            response: raw.response,
        })
    }
}

impl<B> fmt::Debug for DesktopNotificationClient<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DesktopNotificationClient")
            .field("application_id", &self.application_id)
            .field("shutting_down", &self.shutting_down)
            .field("owner_generation", &self.owner_generation)
            .field("owner_available", &self.owner_available)
            .field("pending", &self.pending.len())
            .field("current", &self.current.len())
            .field("active", &self.active.len())
            .field("delivered", &self.delivered.len())
            .field("deferred", &self.deferred.len())
            .finish_non_exhaustive()
    }
}

/// Coalesced wake indicating that notification events may be ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NotificationWake {
    /// Completion-signal generation observed by the subscription.
    pub generation: u64,
}

#[derive(Clone)]
struct NotificationWakeSubscription {
    signal: CompletionSignal,
}

impl Hash for NotificationWakeSubscription {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.signal.identity().hash(state);
    }
}

/// Feature-off backend that reports desktop notifications as unavailable.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableNotificationBackend;

impl NotificationBackend for UnavailableNotificationBackend {
    fn submit_notification(
        &mut self,
        _command: NotificationBackendCommand,
    ) -> Result<NotificationOperationId, NotificationBackendError> {
        Err(NotificationBackendError::Unavailable)
    }

    fn poll_notification_event(&mut self) -> Option<NotificationBackendEvent> {
        None
    }
}

/// Deterministic bounded backend for reducer and transport tests.
#[derive(Debug)]
pub struct MockNotificationBackend {
    next_operation: u64,
    owner_generation: u64,
    available: bool,
    submit_error: Option<NotificationBackendError>,
    submitted: VecDeque<(NotificationOperationId, NotificationBackendCommand)>,
    events: VecDeque<NotificationBackendEvent>,
    signal: CompletionSignal,
    dropped_actions: u64,
}

impl Default for MockNotificationBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockNotificationBackend {
    /// Creates an available mock at owner generation 1.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_operation: 0,
            owner_generation: 1,
            available: true,
            submit_error: None,
            submitted: VecDeque::new(),
            events: VecDeque::new(),
            signal: CompletionSignal::new(),
            dropped_actions: 0,
        }
    }

    /// Configures a persistent immediate submission error.
    pub fn set_submit_error(&mut self, error: Option<NotificationBackendError>) {
        self.submit_error = error;
    }

    /// Submitted commands in order.
    #[must_use]
    pub fn submitted(&self) -> &VecDeque<(NotificationOperationId, NotificationBackendCommand)> {
        &self.submitted
    }

    /// Completes a submitted command with `outcome`.
    pub fn complete(
        &mut self,
        operation: NotificationOperationId,
        outcome: NotificationBackendOutcome,
    ) {
        self.events.push_back(NotificationBackendEvent::Operation {
            operation,
            owner_generation: self.owner_generation,
            outcome,
        });
        self.signal.notify();
    }

    /// Emits a callback from the current owner generation.
    pub fn invoke(
        &mut self,
        notification_id: impl Into<String>,
        action_id: impl Into<String>,
        target: Option<NotificationActionTarget>,
        activation_token: Option<String>,
        response: Option<String>,
    ) {
        self.invoke_for_generation(
            self.owner_generation,
            notification_id,
            action_id,
            target,
            activation_token,
            response,
        );
    }

    /// Emits a callback tagged with an explicit owner generation.
    pub fn invoke_for_generation(
        &mut self,
        owner_generation: u64,
        notification_id: impl Into<String>,
        action_id: impl Into<String>,
        target: Option<NotificationActionTarget>,
        activation_token: Option<String>,
        response: Option<String>,
    ) {
        if self
            .events
            .iter()
            .filter(|event| matches!(event, NotificationBackendEvent::Action(_)))
            .count()
            == NOTIFICATION_ACTION_QUEUE_CAPACITY
        {
            self.dropped_actions = self.dropped_actions.saturating_add(1);
            self.signal.notify();
            return;
        }
        self.events.push_back(NotificationBackendEvent::Action(
            NotificationBackendAction {
                notification_id: notification_id.into(),
                action_id: action_id.into(),
                target,
                activation_token: activation_token
                    .map(crate::activation::InboundActivationToken::from),
                response,
                malformed: false,
                owner_generation,
                received_at: Instant::now(),
            },
        ));
        self.signal.notify();
    }

    /// Emits a desktop closure of `notification_id` from `owner_generation`.
    pub fn close(
        &mut self,
        notification_id: &NotificationId,
        owner_generation: u64,
        reason: NotificationCloseReason,
    ) {
        self.events.push_back(NotificationBackendEvent::Closed {
            notification_id: notification_id.clone(),
            owner_generation,
            reason,
        });
        self.signal.notify();
    }

    /// Simulates service loss or replacement.
    pub fn restart(&mut self, available: bool) {
        self.owner_generation = self.owner_generation.saturating_add(1);
        self.available = available;
        self.events
            .push_back(NotificationBackendEvent::OwnerChanged {
                generation: self.owner_generation,
                available,
            });
        self.signal.notify();
    }
}

impl NotificationBackend for MockNotificationBackend {
    fn submit_notification(
        &mut self,
        command: NotificationBackendCommand,
    ) -> Result<NotificationOperationId, NotificationBackendError> {
        if let Some(error) = self.submit_error.clone() {
            return Err(error);
        }
        if !self.available {
            return Err(NotificationBackendError::Unavailable);
        }
        self.next_operation = self.next_operation.saturating_add(1);
        let id = NotificationOperationId::new(self.next_operation);
        if self.submitted.len() == MAX_PENDING_NOTIFICATION_OPERATIONS {
            self.submitted.pop_front();
        }
        self.submitted.push_back((id, command));
        Ok(id)
    }

    fn poll_notification_event(&mut self) -> Option<NotificationBackendEvent> {
        self.events.pop_front()
    }

    fn notification_completion_signal(&self) -> Option<CompletionSignal> {
        Some(self.signal.clone())
    }

    fn notification_stats(&self) -> NotificationBackendStats {
        NotificationBackendStats {
            pending_actions: self
                .events
                .iter()
                .filter(|event| matches!(event, NotificationBackendEvent::Action(_)))
                .count(),
            action_capacity: NOTIFICATION_ACTION_QUEUE_CAPACITY,
            dropped_actions: self.dropped_actions,
            rejected_actions: 0,
        }
    }
}

#[cfg(test)]
mod tests;
