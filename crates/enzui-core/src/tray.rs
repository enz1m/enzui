//! A system tray item: `org.kde.StatusNotifierItem` with a
//! `com.canonical.dbusmenu` menu.
//!
//! [`Tray::new`] puts a [`TrayItem`] (icon, title, tooltip, status and menu)
//! in the panel's tray and [`Tray::subscription`] delivers what the user does
//! with it as [`TrayEvent`]s. Call [`Tray::set`] with the item the app wants
//! shown on every update: only changes reach the panel. The item lives on its
//! own D-Bus thread and leaves the tray when the [`Tray`] is dropped.
//!
//! KDE Plasma, Xfce, Cinnamon, MATE, Budgie, waybar and swaybar host
//! status-notifier items; GNOME does only with the AppIndicator extension.
//! [`Tray::state`] reports [`TrayState::NoHost`] when nothing hosts them, so
//! the app can keep its window instead of hiding into a tray nobody sees.
//! Flatpak apps need `--talk-name=org.kde.StatusNotifierWatcher` and to own
//! `org.kde.StatusNotifierItem-*` (`--own-name=org.kde.StatusNotifierItem-*`).

use iced_futures::futures::StreamExt as _;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Type, Value};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";

/// What the tray item is doing, as the panel shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TrayStatus {
    /// Shown normally.
    #[default]
    Active,
    /// Present but unimportant: panels may tuck it away.
    Passive,
    /// Asking for the user (a new message, a finished task).
    NeedsAttention,
}

impl TrayStatus {
    const fn name(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Passive => "Passive",
            Self::NeedsAttention => "NeedsAttention",
        }
    }
}

/// One icon image on the bus: width, height, ARGB32 pixels.
type Pixmap = (i32, i32, Vec<u8>);
/// A tooltip on the bus: icon name, icon images, heading, body.
type ToolTip = (String, Vec<Pixmap>, String, String);

/// The tray icon's image.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TrayIcon {
    /// An icon from the user's icon theme, such as `mail-unread-symbolic`.
    Named(String),
    /// Straight RGBA pixels, row by row.
    Rgba {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// `width * height * 4` bytes.
        pixels: Vec<u8>,
    },
}

impl TrayIcon {
    fn name(&self) -> &str {
        match self {
            Self::Named(name) => name,
            Self::Rgba { .. } => "",
        }
    }

    /// The pixmap list: ARGB32 in network byte order, as the protocol wants.
    fn pixmaps(&self) -> Vec<Pixmap> {
        match self {
            Self::Named(_) => Vec::new(),
            Self::Rgba {
                width,
                height,
                pixels,
            } => {
                let argb = pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|rgba| [rgba[3], rgba[0], rgba[1], rgba[2]])
                    .collect();
                vec![(
                    i32::try_from(*width).unwrap_or(0),
                    i32::try_from(*height).unwrap_or(0),
                    argb,
                )]
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum MenuKind {
    Action,
    Check(bool),
    Radio(bool),
    Separator,
    Submenu(Vec<TrayMenuItem>),
}

/// One row of the tray item's menu.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrayMenuItem {
    id: String,
    label: String,
    enabled: bool,
    icon: Option<String>,
    kind: MenuKind,
}

impl TrayMenuItem {
    fn new(id: impl Into<String>, label: impl Into<String>, kind: MenuKind) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled: true,
            icon: None,
            kind,
        }
    }

    /// A row that sends [`TrayEvent::MenuItem`] with `id` when chosen.
    pub fn action(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(id, label, MenuKind::Action)
    }

    /// A row with a check mark. Choosing it sends [`TrayEvent::MenuItem`];
    /// the app flips its state and sets the item again.
    pub fn check(id: impl Into<String>, label: impl Into<String>, checked: bool) -> Self {
        Self::new(id, label, MenuKind::Check(checked))
    }

    /// A row of a one-of-several choice, marked when `selected`.
    pub fn radio(id: impl Into<String>, label: impl Into<String>, selected: bool) -> Self {
        Self::new(id, label, MenuKind::Radio(selected))
    }

    /// A separating line.
    #[must_use]
    pub fn separator() -> Self {
        Self::new("", "", MenuKind::Separator)
    }

    /// A row opening `items`.
    pub fn submenu(label: impl Into<String>, items: Vec<Self>) -> Self {
        Self::new("", label, MenuKind::Submenu(items))
    }

    /// Greys the row out when `enabled` is false.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// An icon from the theme beside the label.
    #[must_use]
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = Some(name.into());
        self
    }

    fn properties(&self) -> HashMap<String, OwnedValue> {
        let mut properties = HashMap::new();
        let mut put = |key: &str, value: Value<'_>| {
            if let Ok(value) = value.try_into_owned() {
                let _ = properties.insert(key.to_owned(), value);
            }
        };
        if self.kind == MenuKind::Separator {
            put("type", "separator".into());
            return properties;
        }
        put("label", self.label.as_str().into());
        put("enabled", self.enabled.into());
        if let Some(icon) = &self.icon {
            put("icon-name", icon.as_str().into());
        }
        match &self.kind {
            MenuKind::Check(on) => {
                put("toggle-type", "checkmark".into());
                put("toggle-state", i32::from(*on).into());
            }
            MenuKind::Radio(on) => {
                put("toggle-type", "radio".into());
                put("toggle-state", i32::from(*on).into());
            }
            MenuKind::Submenu(_) => put("children-display", "submenu".into()),
            MenuKind::Action | MenuKind::Separator => {}
        }
        properties
    }
}

/// Everything the tray shows for the app.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrayItem {
    /// A stable name for the item, usually the application ID.
    pub id: String,
    /// The application's name.
    pub title: String,
    /// The icon.
    pub icon: TrayIcon,
    /// The tooltip's heading; empty for none.
    pub tooltip: String,
    /// The tooltip's body text under the heading.
    pub tooltip_body: String,
    /// Active, passive or asking for attention.
    pub status: TrayStatus,
    /// The menu, shown on a secondary click.
    pub menu: Vec<TrayMenuItem>,
}

impl TrayItem {
    /// An active item with a themed icon, no tooltip and no menu.
    pub fn new(id: impl Into<String>, title: impl Into<String>, icon: TrayIcon) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            icon,
            tooltip: String::new(),
            tooltip_body: String::new(),
            status: TrayStatus::Active,
            menu: Vec::new(),
        }
    }

    /// With a tooltip heading and body.
    #[must_use]
    pub fn tooltip(mut self, heading: impl Into<String>, body: impl Into<String>) -> Self {
        self.tooltip = heading.into();
        self.tooltip_body = body.into();
        self
    }

    /// With `status`.
    #[must_use]
    pub const fn status(mut self, status: TrayStatus) -> Self {
        self.status = status;
        self
    }

    /// With a menu.
    #[must_use]
    pub fn menu(mut self, menu: Vec<TrayMenuItem>) -> Self {
        self.menu = menu;
        self
    }
}

/// What the user did with the tray item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TrayEvent {
    /// The icon was clicked (or activated from the keyboard): usually, show
    /// or raise the main window.
    Activated,
    /// The icon was middle-clicked.
    SecondaryActivated,
    /// The wheel turned over the icon by `delta` steps.
    Scrolled {
        /// Steps; positive is down or right.
        delta: i32,
        /// Whether the scroll was horizontal.
        horizontal: bool,
    },
    /// The menu row with this ID was chosen.
    MenuItem(String),
    /// The host asked the app to show its own menu: it does not draw
    /// [`TrayItem::menu`] itself (swaybar, for one). Raise the window, or
    /// offer the same choices there.
    MenuRequested,
}

/// Whether the item is in a tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayState {
    /// Connecting to the bus and registering.
    Starting,
    /// A tray host shows the item.
    Shown,
    /// Nothing on the desktop hosts tray items right now; the item appears
    /// if a host starts later.
    NoHost,
    /// The session bus could not be reached.
    Unavailable,
}

enum Command {
    Set(TrayItem),
}

/// A tray item owned by the app; see the module docs.
#[derive(Debug)]
pub struct Tray {
    shown: TrayItem,
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    events: Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<TrayEvent>>>,
    state: Arc<Mutex<TrayState>>,
    key: u64,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Command")
    }
}

impl Tray {
    /// Puts `item` in the tray on the session bus.
    #[must_use]
    pub fn new(item: TrayItem) -> Self {
        Self::start(item, None)
    }

    /// Puts `item` in the tray on the bus at `address`.
    #[must_use]
    pub fn on_bus(item: TrayItem, address: impl Into<String>) -> Self {
        Self::start(item, Some(address.into()))
    }

    fn start(item: TrayItem, address: Option<String>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let key = NEXT.fetch_add(1, Ordering::Relaxed);
        let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        let (events, event_receiver) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(Mutex::new(TrayState::Starting));
        let (thread_state, thread_item) = (Arc::clone(&state), item.clone());
        let spawned = std::thread::Builder::new()
            .name("enzui-tray".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    set_state(&thread_state, TrayState::Unavailable);
                    return;
                };
                runtime.block_on(async {
                    let served = serve(
                        address,
                        key,
                        thread_item,
                        receiver,
                        events,
                        Arc::clone(&thread_state),
                    )
                    .await;
                    if let Err(error) = served {
                        log::info!(target: "enzui::tray", "no tray item: {error}");
                        set_state(&thread_state, TrayState::Unavailable);
                    }
                });
            });
        if spawned.is_err() {
            set_state(&state, TrayState::Unavailable);
        }
        Self {
            shown: item,
            commands,
            events: Arc::new(tokio::sync::Mutex::new(event_receiver)),
            state,
            key,
        }
    }

    /// Shows `item`, sending only what changed.
    pub fn set(&mut self, item: TrayItem) {
        if item != self.shown {
            self.shown = item.clone();
            let _ = self.commands.send(Command::Set(item));
        }
    }

    /// The item as last set.
    #[must_use]
    pub const fn item(&self) -> &TrayItem {
        &self.shown
    }

    /// Whether a tray shows the item.
    #[must_use]
    pub fn state(&self) -> TrayState {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What the user does with the item.
    pub fn subscription(&self) -> iced_futures::Subscription<TrayEvent> {
        struct Recipe {
            key: u64,
            events: Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<TrayEvent>>>,
        }
        impl iced_futures::subscription::Recipe for Recipe {
            type Output = TrayEvent;

            fn hash(&self, state: &mut iced_futures::subscription::Hasher) {
                use std::hash::Hash;
                ("enzui-tray", self.key).hash(state);
            }

            fn stream(
                self: Box<Self>,
                _input: iced_futures::subscription::EventStream,
            ) -> iced_futures::BoxStream<Self::Output> {
                Box::pin(iced_futures::futures::stream::unfold(
                    self.events,
                    |events| async move {
                        let event = events.lock().await.recv().await;
                        event.map(|event| (event, events))
                    },
                ))
            }
        }
        iced_futures::subscription::from_recipe(Recipe {
            key: self.key,
            events: Arc::clone(&self.events),
        })
    }
}

fn set_state(state: &Mutex<TrayState>, value: TrayState) {
    *state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
}

/// The item with its menu numbered for dbusmenu.
#[derive(Debug)]
struct Model {
    item: TrayItem,
    revision: u32,
    /// Menu rows by dbusmenu ID; 0 is the root.
    rows: HashMap<i32, (Option<TrayMenuItem>, Vec<i32>)>,
}

impl Model {
    fn new(item: TrayItem) -> Self {
        let mut model = Self {
            item,
            revision: 1,
            rows: HashMap::new(),
        };
        model.number();
        model
    }

    fn number(&mut self) {
        fn add(
            rows: &mut HashMap<i32, (Option<TrayMenuItem>, Vec<i32>)>,
            next: &mut i32,
            items: &[TrayMenuItem],
        ) -> Vec<i32> {
            items
                .iter()
                .map(|item| {
                    *next += 1;
                    let id = *next;
                    let children = match &item.kind {
                        MenuKind::Submenu(items) => add(rows, next, items),
                        _ => Vec::new(),
                    };
                    let _ = rows.insert(id, (Some(item.clone()), children));
                    id
                })
                .collect()
        }
        self.rows.clear();
        let mut next = 0;
        let menu = self.item.menu.clone();
        let root = add(&mut self.rows, &mut next, &menu);
        let _ = self.rows.insert(0, (None, root));
    }

    fn properties(&self, id: i32, names: &[String]) -> Option<HashMap<String, OwnedValue>> {
        let (row, children) = self.rows.get(&id)?;
        let mut properties = match row {
            Some(item) => item.properties(),
            None => HashMap::new(),
        };
        if row.is_none()
            && !children.is_empty()
            && let Ok(value) = Value::from("submenu").try_into_owned()
        {
            let _ = properties.insert("children-display".to_owned(), value);
        }
        if !names.is_empty() {
            properties.retain(|name, _| names.contains(name));
        }
        Some(properties)
    }

    fn layout(&self, id: i32, depth: i32, names: &[String]) -> Option<Layout> {
        let properties = self.properties(id, names)?;
        let children = if depth == 0 {
            Vec::new()
        } else {
            self.rows[&id]
                .1
                .iter()
                .filter_map(|child| self.layout(*child, depth - 1, names))
                .filter_map(|layout| OwnedValue::try_from(Value::from(layout)).ok())
                .collect()
        };
        Some(Layout {
            id,
            properties,
            children,
        })
    }
}

/// A dbusmenu layout node, `(ia{sv}av)`.
#[derive(Debug, serde::Serialize, Type, Value)]
struct Layout {
    id: i32,
    properties: HashMap<String, OwnedValue>,
    children: Vec<OwnedValue>,
}

/// The D-Bus objects. zbus generates a public signals trait for each, so they
/// stay in a private module.
mod bus {
    use super::{
        Arc, HashMap, Layout, MENU_PATH, Model, Mutex, OwnedObjectPath, OwnedValue, Pixmap,
        SignalEmitter, ToolTip, TrayEvent, TrayItem,
    };

    pub(super) struct ItemObject {
        pub(super) model: Arc<Mutex<Model>>,
        pub(super) events: tokio::sync::mpsc::UnboundedSender<TrayEvent>,
    }

    impl ItemObject {
        fn read<T>(&self, read: impl FnOnce(&TrayItem) -> T) -> T {
            read(
                &self
                    .model
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .item,
            )
        }
    }

    #[zbus::interface(name = "org.kde.StatusNotifierItem", spawn = false)]
    impl ItemObject {
        #[zbus(property)]
        fn category(&self) -> &str {
            "ApplicationStatus"
        }

        #[zbus(property)]
        fn id(&self) -> String {
            self.read(|item| item.id.clone())
        }

        #[zbus(property)]
        fn title(&self) -> String {
            self.read(|item| item.title.clone())
        }

        #[zbus(property)]
        fn status(&self) -> &str {
            self.read(|item| item.status.name())
        }

        #[zbus(property)]
        fn window_id(&self) -> i32 {
            0
        }

        #[zbus(property)]
        fn icon_theme_path(&self) -> &str {
            ""
        }

        #[zbus(property)]
        fn icon_name(&self) -> String {
            self.read(|item| item.icon.name().to_owned())
        }

        #[zbus(property)]
        fn icon_pixmap(&self) -> Vec<Pixmap> {
            self.read(|item| item.icon.pixmaps())
        }

        #[zbus(property)]
        fn overlay_icon_name(&self) -> &str {
            ""
        }

        #[zbus(property)]
        fn overlay_icon_pixmap(&self) -> Vec<Pixmap> {
            Vec::new()
        }

        #[zbus(property)]
        fn attention_icon_name(&self) -> String {
            self.icon_name()
        }

        #[zbus(property)]
        fn attention_icon_pixmap(&self) -> Vec<Pixmap> {
            self.icon_pixmap()
        }

        #[zbus(property)]
        fn attention_movie_name(&self) -> &str {
            ""
        }

        #[zbus(property)]
        fn tool_tip(&self) -> ToolTip {
            self.read(|item| {
                (
                    String::new(),
                    Vec::new(),
                    item.tooltip.clone(),
                    item.tooltip_body.clone(),
                )
            })
        }

        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            false
        }

        #[zbus(property)]
        fn menu(&self) -> OwnedObjectPath {
            OwnedObjectPath::try_from(MENU_PATH).unwrap_or_default()
        }

        fn context_menu(&self, _x: i32, _y: i32) {
            let _ = self.events.send(TrayEvent::MenuRequested);
        }

        fn activate(&self, _x: i32, _y: i32) {
            let _ = self.events.send(TrayEvent::Activated);
        }

        fn secondary_activate(&self, _x: i32, _y: i32) {
            let _ = self.events.send(TrayEvent::SecondaryActivated);
        }

        fn scroll(&self, delta: i32, orientation: &str) {
            let _ = self.events.send(TrayEvent::Scrolled {
                delta,
                horizontal: orientation.eq_ignore_ascii_case("horizontal"),
            });
        }

        /// The title changed.
        #[zbus(signal)]
        pub(super) async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        /// The icon changed.
        #[zbus(signal)]
        pub(super) async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        /// The attention icon changed.
        #[zbus(signal)]
        pub(super) async fn new_attention_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        /// The tooltip changed.
        #[zbus(signal)]
        pub(super) async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        /// The status changed.
        #[zbus(signal)]
        pub(super) async fn new_status(
            emitter: &SignalEmitter<'_>,
            status: &str,
        ) -> zbus::Result<()>;
    }

    pub(super) struct MenuObject {
        pub(super) model: Arc<Mutex<Model>>,
        pub(super) events: tokio::sync::mpsc::UnboundedSender<TrayEvent>,
    }

    impl MenuObject {
        fn model(&self) -> std::sync::MutexGuard<'_, Model> {
            self.model
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        /// Delivers a click on row `id`; false when there is no such row.
        fn deliver(&self, id: i32, event: &str) -> bool {
            let chosen = {
                let model = self.model();
                let Some((row, _)) = model.rows.get(&id) else {
                    return false;
                };
                row.as_ref()
                    .filter(|row| row.enabled && !row.id.is_empty())
                    .map(|row| row.id.clone())
            };
            if event == "clicked"
                && let Some(id) = chosen
            {
                let _ = self.events.send(TrayEvent::MenuItem(id));
            }
            true
        }
    }

    #[zbus::interface(name = "com.canonical.dbusmenu", spawn = false)]
    impl MenuObject {
        #[zbus(property)]
        fn version(&self) -> u32 {
            3
        }

        #[zbus(property)]
        fn text_direction(&self) -> &str {
            "ltr"
        }

        #[zbus(property)]
        fn status(&self) -> &str {
            "normal"
        }

        #[zbus(property)]
        fn icon_theme_path(&self) -> Vec<String> {
            Vec::new()
        }

        fn get_layout(
            &self,
            parent_id: i32,
            recursion_depth: i32,
            property_names: Vec<String>,
        ) -> zbus::fdo::Result<(u32, Layout)> {
            let model = self.model();
            model
                .layout(parent_id, recursion_depth, &property_names)
                .map(|layout| (model.revision, layout))
                .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no menu item {parent_id}")))
        }

        fn get_group_properties(
            &self,
            ids: Vec<i32>,
            property_names: Vec<String>,
        ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
            let model = self.model();
            let ids = if ids.is_empty() {
                model.rows.keys().copied().collect()
            } else {
                ids
            };
            ids.into_iter()
                .filter_map(|id| {
                    model
                        .properties(id, &property_names)
                        .map(|properties| (id, properties))
                })
                .collect()
        }

        fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
            self.model()
                .properties(id, std::slice::from_ref(&name))
                .and_then(|mut properties| properties.remove(&name))
                .ok_or_else(|| {
                    zbus::fdo::Error::InvalidArgs(format!("no {name} on menu item {id}"))
                })
        }

        fn event(&self, id: i32, event_id: &str, _data: OwnedValue, _timestamp: u32) {
            let _ = self.deliver(id, event_id);
        }

        fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
            events
                .into_iter()
                .filter(|(id, event, _, _)| !self.deliver(*id, event))
                .map(|(id, ..)| id)
                .collect()
        }

        fn about_to_show(&self, _id: i32) -> bool {
            false
        }

        fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
            (Vec::new(), Vec::new())
        }

        /// The menu changed from `parent` down.
        #[zbus(signal)]
        pub(super) async fn layout_updated(
            emitter: &SignalEmitter<'_>,
            revision: u32,
            parent: i32,
        ) -> zbus::Result<()>;
    }
}
use bus::{ItemObject, MenuObject};

async fn register(connection: &zbus::Connection, name: &str) -> bool {
    connection
        .call_method(
            Some(WATCHER),
            "/StatusNotifierWatcher",
            Some(WATCHER),
            "RegisterStatusNotifierItem",
            &(name,),
        )
        .await
        .is_ok()
}

async fn serve(
    address: Option<String>,
    key: u64,
    item: TrayItem,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<Command>,
    events: tokio::sync::mpsc::UnboundedSender<TrayEvent>,
    state: Arc<Mutex<TrayState>>,
) -> zbus::Result<()> {
    let model = Arc::new(Mutex::new(Model::new(item)));
    let name = format!(
        "org.kde.StatusNotifierItem-{}-{}",
        std::process::id(),
        key + 1
    );
    let builder = match &address {
        Some(address) => zbus::connection::Builder::address(address.as_str())?,
        None => zbus::connection::Builder::session()?,
    };
    let connection = builder
        .name(name.as_str())?
        .serve_at(
            ITEM_PATH,
            ItemObject {
                model: Arc::clone(&model),
                events: events.clone(),
            },
        )?
        .serve_at(
            MENU_PATH,
            MenuObject {
                model: Arc::clone(&model),
                events,
            },
        )?
        .build()
        .await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    let mut watchers = bus
        .receive_name_owner_changed_with_args(&[(0, WATCHER)])
        .await?;
    let shown = |registered| {
        if registered {
            TrayState::Shown
        } else {
            TrayState::NoHost
        }
    };
    set_state(&state, shown(register(&connection, &name).await));
    let item_emitter = SignalEmitter::new(&connection, ITEM_PATH)?;
    let menu_emitter = SignalEmitter::new(&connection, MENU_PATH)?;
    loop {
        use iced_futures::futures::future::{Either, select};
        match select(Box::pin(commands.recv()), Box::pin(watchers.next())).await {
            // Dropping the Tray closes the channel: leave the bus.
            Either::Left((None, _)) | Either::Right((None, _)) => return Ok(()),
            Either::Left((Some(Command::Set(item)), _)) => {
                let before = {
                    let mut model = model
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let before = std::mem::replace(&mut model.item, item);
                    if before.menu != model.item.menu {
                        model.revision += 1;
                        model.number();
                    }
                    before
                };
                let (after, revision) = {
                    let model = model
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    (model.item.clone(), model.revision)
                };
                if before.title != after.title {
                    let _ = ItemObject::new_title(&item_emitter).await;
                }
                if before.icon != after.icon {
                    let _ = ItemObject::new_icon(&item_emitter).await;
                    let _ = ItemObject::new_attention_icon(&item_emitter).await;
                }
                if (&before.tooltip, &before.tooltip_body) != (&after.tooltip, &after.tooltip_body)
                {
                    let _ = ItemObject::new_tool_tip(&item_emitter).await;
                }
                if before.status != after.status {
                    let _ = ItemObject::new_status(&item_emitter, after.status.name()).await;
                }
                if before.menu != after.menu {
                    let _ = MenuObject::layout_updated(&menu_emitter, revision, 0).await;
                }
            }
            Either::Right((Some(change), _)) => {
                // A tray host (re)started or went away.
                let appeared = change.args().is_ok_and(|args| args.new_owner().is_some());
                let registered = appeared && register(&connection, &name).await;
                set_state(&state, shown(registered));
            }
        }
    }
}
