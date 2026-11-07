//! Roles and value types used by [`Semantics`](super::Semantics).

/// A semantic role supported by enzUI accessibility adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// A push button (`EnzButton`, window controls, toolbar icons).
    Button,
    /// A hyperlink (`EnzLink`).
    Link,
    /// A checkbox that may have checked, unchecked, or mixed state (`EnzCheckbox`).
    CheckBox,
    /// An on/off switch (`EnzToggle`).
    Switch,
    /// A single radio option (`EnzRadioButton`).
    RadioButton,
    /// A group of mutually-exclusive radio options (`EnzRadioGroup`).
    RadioGroup,
    /// A slider over a numeric range (`EnzSlider`).
    Slider,
    /// A bounded numeric entry that can be stepped or set directly.
    SpinButton,
    /// A single-line text entry (`EnzTextField`, `EnzSearchField`).
    TextInput,
    /// A password / masked entry (`EnzSecureField`).
    PasswordInput,
    /// A determinate/indeterminate progress indicator (`EnzProgressBar`).
    ProgressIndicator,
    /// A selectable list/menu row (`EnzMenuRow`).
    ListItem,
    /// A tab in a tab strip (`EnzTabPill`, `EnzViewSwitcher`).
    Tab,
    /// A tab-list container with mutually exclusive tabs and roving keyboard focus.
    TabList,
    /// A non-interactive text label (`EnzText`).
    Label,
    /// A read-only document surface whose descendants expose the currently
    /// materialized text range. Maps to AccessKit's `Document` role.
    Document,
    /// A section heading used for document-outline navigation.
    ///
    /// Its outline level is carried separately by [`Semantics::level`](super::Semantics::level).
    Heading,
    /// A non-interactive image / picture (`EnzAvatar` photo). Carries the
    /// image's accessible name; the app owns the description.
    Image,
    /// An expandable disclosure control (`EnzDisclosureButton`) — a triangle /
    /// chevron that shows or hides a section.
    Disclosure,
    /// A focusable, keyboard-adjustable separator carrying a numeric split value and orientation.
    Splitter,
    /// A popup command menu containing [`MenuItem`](Role::MenuItem) rows.
    Menu,
    /// A command row in a menu that fires rather than representing persistent selection.
    MenuItem,
    /// A checkable command row in a menu.
    MenuItemCheckBox,
    /// A two-dimensional grid containing rows and addressable cells.
    Grid,
    /// A row within a [`Grid`](Role::Grid).
    GridRow,
    /// An addressable grid cell carrying a [`GridPosition`] for coordinate announcements.
    GridCell,
    /// A header for a column in a grid or table.
    ColumnHeader,
    /// A hierarchical container of tree items with roving keyboard focus.
    Tree,
    /// A tree row carrying hierarchy, set position, selection, and optional expansion state.
    TreeItem,
    /// A date entry field with date-specific assistive-technology semantics.
    DateInput,
    /// A dialog surface whose modality is set independently with [`Semantics::modal`](super::Semantics::modal).
    Dialog,
    /// A dialog announced urgently because it requires a response.
    AlertDialog,
    /// A non-editable select control that owns a [`ListBox`](Role::ListBox) popup and exposes its value and expansion state.
    ComboBox,
    /// An editable combo box whose text field retains focus while [`Semantics::active_descendant`](super::Semantics::active_descendant) identifies a highlighted suggestion.
    EditableComboBox,
    /// A list of persistent selectable options owned by a combo box or select popup.
    ListBox,
    /// A selectable list-box row carrying selection and set-position state.
    ListBoxOption,
    /// A generic grouping container.
    Group,
    /// A non-dialog advisory live region that defaults to [`Live::Polite`].
    Status,
    /// An urgent non-dialog live region that defaults to [`Live::Assertive`].
    Alert,
    /// A horizontal bar of menus.
    MenuBar,
    /// A menu item that is one choice of several (radio).
    MenuItemRadio,
    /// The content a [`Role::Tab`] shows.
    TabPanel,
    /// A row of controls acting on the content around it.
    Toolbar,
    /// A short description that appears for a control, usually on hover.
    Tooltip,
    /// A visual divider between groups of items.
    Separator,
    /// Landmark: a set of links or controls to move around the application.
    Navigation,
    /// Landmark: the main content.
    Main,
    /// Landmark: a search area.
    Search,
    /// Landmark: the header of the window or page.
    Banner,
    /// Landmark: the footer of the window or page.
    ContentInfo,
    /// Landmark: content that complements the main content (a sidebar).
    Complementary,
    /// Landmark: a labelled region worth navigating to.
    Region,
    /// Landmark: a form.
    Form,
    /// A terminal: its screen text, a caret, and output announced as it
    /// arrives.
    Terminal,
}

/// The politeness requested for live-region announcements.
///
/// Absence on [`Semantics`](super::Semantics) inherits from an ancestor, while an explicit value overrides that inheritance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Live {
    /// Never announce automatically. The region is still navigable.
    Off,
    /// Announces at a convenient pause and is the default for [`Role::Status`].
    Polite,
    /// Announces immediately and is the default for [`Role::Alert`].
    Assertive,
}

/// A zero-based grid position together with the grid’s row and column counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GridPosition {
    /// 0-based row.
    pub row: usize,
    /// 0-based column.
    pub column: usize,
    /// Total rows in the grid.
    pub row_count: usize,
    /// Total columns in the grid.
    pub column_count: usize,
}

/// A 1-based item position and total count within an ordered set.
///
/// AccessKit conversion subtracts one from `index` at the adapter boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SetPosition {
    /// 1-based index of this item within the set.
    pub index: usize,
    /// Total number of items in the set.
    pub count: usize,
}

/// Tri-state toggle value for checkboxes / switches, mirroring
/// `accesskit::Toggled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Toggle {
    /// Off / unchecked.
    False,
    /// On / checked.
    True,
    /// Indeterminate / mixed (tri-state checkbox).
    Mixed,
}

/// The consequence class of invoking an action, also rendered into the accessible description when no native property exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionDisposition {
    /// Invoking the action can irreversibly remove or destroy user data.
    Destructive,
}

impl ActionDisposition {
    /// The AT-visible phrase used when no native platform property exists.
    pub fn announcement(self) -> String {
        match self {
            Self::Destructive => crate::localization::text("enzui-semantics-destructive"),
        }
    }
}

/// The current sort state of a sortable column.
///
/// `None` on [`Semantics::sort`](super::Semantics::sort) means not sortable, while [`SortDirection::Unsorted`] means sortable but not currently sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortDirection {
    /// Sortable, but not currently the sorted column (`aria-sort="none"`).
    Unsorted,
    /// Sorted in ascending order.
    Ascending,
    /// Sorted in descending order.
    Descending,
    /// Sorted by a custom or otherwise non-directional order.
    Other,
}

impl SortDirection {
    /// Returns the user-facing sort phrase used on platforms without an audible native sort property.
    pub fn announcement(self) -> String {
        match self {
            // Distinguish a sortable unsorted column from a column with no sort capability.
            SortDirection::Unsorted => {
                crate::localization::text("enzui-semantics-sortable-unsorted")
            }
            SortDirection::Ascending => {
                crate::localization::text("enzui-semantics-sorted-ascending")
            }
            SortDirection::Descending => {
                crate::localization::text("enzui-semantics-sorted-descending")
            }
            // `Other` is sorted without a named direction.
            SortDirection::Other => crate::localization::text("enzui-semantics-sorted"),
        }
    }
}

/// A named rung in the document outline, mapped to `aria-level` values 1 through 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HeadingLevel {
    /// Level 1, representing the page or pane title.
    Page,
    /// `aria-level` 2 — a top-level region of the page.
    Section,
    /// `aria-level` 3 — a group nested inside a [`Section`](Self::Section).
    Subsection,
    /// `aria-level` 4 — the deepest rung expressed here.
    Detail,
}

impl HeadingLevel {
    /// The 1-based `aria-level` integer this rung publishes.
    pub fn level(self) -> usize {
        match self {
            Self::Page => 1,
            Self::Section => 2,
            Self::Subsection => 3,
            Self::Detail => 4,
        }
    }

    /// Returns the rung for levels 1 through 4, or `None` for any other value.
    pub fn from_level(level: usize) -> Option<Self> {
        match level {
            1 => Some(Self::Page),
            2 => Some(Self::Section),
            3 => Some(Self::Subsection),
            4 => Some(Self::Detail),
            _ => None,
        }
    }
}

/// Orientation for controls that have one (sliders, radio groups),
/// mirroring `accesskit::Orientation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// Laid out horizontally.
    Horizontal,
    /// Laid out vertically.
    Vertical,
}

/// The numeric value contract for range widgets (slider, progress bar).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericValue {
    /// Current value.
    pub value: f64,
    /// Inclusive minimum.
    pub min: f64,
    /// Inclusive maximum.
    pub max: f64,
    /// Step increment, if the control snaps.
    pub step: Option<f64>,
}
