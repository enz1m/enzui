//! Window-chrome commands and their per-window runtime tasks.
//!
//! Titlebar widgets publish [`WindowCommand`] messages; the application's
//! update handler calls [`WindowCommand::into_task`] with the target window id.
//! ToggleMaximize uses the runtime's current state, so it also restores an
//! already maximized window. [`ResizeEdge`] supplies native resize directions,
//! cursor shapes and stable identity suffixes.

/// Edges and corners for native window resizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResizeEdge {
    /// Top edge (grows/shrinks upward).
    North,
    /// Bottom edge.
    South,
    /// Right edge.
    East,
    /// Left edge.
    West,
    /// Top-right corner.
    NorthEast,
    /// Top-left corner.
    NorthWest,
    /// Bottom-right corner.
    SouthEast,
    /// Bottom-left corner.
    SouthWest,
}

impl ResizeEdge {
    /// Every octant, in a stable order (edges then corners). Used by tests and
    /// by the resize-frame widget's hit classifier.
    pub const ALL: [ResizeEdge; 8] = [
        ResizeEdge::North,
        ResizeEdge::South,
        ResizeEdge::East,
        ResizeEdge::West,
        ResizeEdge::NorthEast,
        ResizeEdge::NorthWest,
        ResizeEdge::SouthEast,
        ResizeEdge::SouthWest,
    ];

    /// Map to the vendored per-window resize `iced::window::Direction`.
    #[must_use]
    pub fn direction(self) -> iced::window::Direction {
        use iced::window::Direction;
        match self {
            ResizeEdge::North => Direction::North,
            ResizeEdge::South => Direction::South,
            ResizeEdge::East => Direction::East,
            ResizeEdge::West => Direction::West,
            ResizeEdge::NorthEast => Direction::NorthEast,
            ResizeEdge::NorthWest => Direction::NorthWest,
            ResizeEdge::SouthEast => Direction::SouthEast,
            ResizeEdge::SouthWest => Direction::SouthWest,
        }
    }

    /// Choose horizontal, vertical or diagonal resize cursors for this edge.
    #[must_use]
    pub fn cursor(self) -> iced::mouse::Interaction {
        use iced::mouse::Interaction;
        match self {
            ResizeEdge::West | ResizeEdge::East => Interaction::ResizingHorizontally,
            ResizeEdge::North | ResizeEdge::South => Interaction::ResizingVertically,
            ResizeEdge::NorthWest | ResizeEdge::SouthEast => Interaction::ResizingDiagonallyDown,
            ResizeEdge::NorthEast | ResizeEdge::SouthWest => Interaction::ResizingDiagonallyUp,
        }
    }

    /// A stable, non-positional logical name for this octant (used as an id
    /// role suffix and in diagnostics).
    #[must_use]
    pub fn id_suffix(self) -> &'static str {
        match self {
            ResizeEdge::North => "north",
            ResizeEdge::South => "south",
            ResizeEdge::East => "east",
            ResizeEdge::West => "west",
            ResizeEdge::NorthEast => "north-east",
            ResizeEdge::NorthWest => "north-west",
            ResizeEdge::SouthEast => "south-east",
            ResizeEdge::SouthWest => "south-west",
        }
    }
}

/// A window-chrome command published by titlebar gesture surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowCommand {
    /// Begin a compositor-owned window move (single drag past the deadband).
    Drag,
    /// Toggle maximized/restored (double-click on the drag region, or the
    /// maximize control).
    ToggleMaximize,
    /// Minimize/iconify the window.
    Minimize,
    /// Close the window.
    Close,
    /// Show the window/system menu (right-press on the drag region).
    ShowSystemMenu,
    /// Begin a compositor-owned window resize from the given edge/corner
    /// (press on one of the shell's invisible CSD resize regions).
    Resize(ResizeEdge),
}

impl WindowCommand {
    /// Build the native runtime task targeting `window`.
    ///
    /// Minimize requests the minimized state; maximize toggles the current
    /// state. The application must return or otherwise execute the task.
    pub fn into_task<Message: Send + 'static>(
        self,
        window: iced::window::Id,
    ) -> iced::Task<Message> {
        match self {
            Self::Drag => iced::window::drag(window),
            Self::ToggleMaximize => iced::window::toggle_maximize(window),
            Self::Minimize => iced::window::minimize(window, true),
            Self::Close => iced::window::close(window),
            Self::ShowSystemMenu => iced::window::show_system_menu(window),
            Self::Resize(edge) => iced::window::drag_resize(window, edge.direction()),
        }
    }

    /// Return the stable identity suffix used by titlebar controls.
    ///
    /// All resize edges share `"resize"`; combine with [`ResizeEdge::id_suffix`]
    /// when identifying individual resize regions.
    #[must_use]
    pub fn id_suffix(self) -> &'static str {
        match self {
            Self::Drag => "drag",
            Self::ToggleMaximize => "maximize",
            Self::Minimize => "minimize",
            Self::Close => "close",
            Self::ShowSystemMenu => "system-menu",
            Self::Resize(_) => "resize",
        }
    }
}
