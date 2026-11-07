use super::{EnzPaneGridState, Orientation, PaneTree, SeparatorModel};
use iced::widget::pane_grid as native;

/// An outer grid edge or the edge of one pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneEdge {
    /// Above the existing region.
    Top,
    /// Before the existing region.
    Left,
    /// After the existing region.
    Right,
    /// Below the existing region.
    Bottom,
}

impl PaneEdge {
    pub(super) fn native(self) -> native::Edge {
        match self {
            Self::Top => native::Edge::Top,
            Self::Left => native::Edge::Left,
            Self::Right => native::Edge::Right,
            Self::Bottom => native::Edge::Bottom,
        }
    }

    fn from_native(edge: native::Edge) -> Self {
        match edge {
            native::Edge::Top => Self::Top,
            native::Edge::Left => Self::Left,
            native::Edge::Right => Self::Right,
            native::Edge::Bottom => Self::Bottom,
        }
    }
}

/// Center drops swap panes; edge drops split the destination region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneRegion {
    /// Exchange positions with the destination pane.
    #[default]
    Center,
    /// Place the source beside the destination pane.
    Edge(PaneEdge),
}

impl PaneRegion {
    fn native(self) -> native::Region {
        match self {
            Self::Center => native::Region::Center,
            Self::Edge(edge) => native::Region::Edge(edge.native()),
        }
    }

    fn from_native(region: native::Region) -> Self {
        match region {
            native::Region::Center => Self::Center,
            native::Region::Edge(edge) => Self::Edge(PaneEdge::from_native(edge)),
        }
    }
}

/// Destination expressed in application keys, independent of native pane IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneDropTarget<T> {
    /// Split the whole grid at this edge.
    Edge(PaneEdge),
    /// Swap with or split one keyed pane.
    Pane {
        /// Destination application key.
        key: T,
        /// Center swap or edge split.
        region: PaneRegion,
    },
}

/// A header drag gesture; only a dropped event requests a tree change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneDragEvent<T> {
    /// A draggable header was pressed.
    Picked {
        /// Source application key.
        key: T,
    },
    /// The source was released over a destination.
    Dropped {
        /// Source application key.
        key: T,
        /// Destination region.
        target: PaneDropTarget<T>,
    },
    /// Release outside a destination, pointer exit, focus loss, or no drag movement.
    Canceled {
        /// Source application key.
        key: T,
    },
}

/// A keyed operation was rejected without changing pane or separator state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKeyError {
    /// A requested key has no pane.
    Unknown,
    /// More than one pane has the same key.
    Ambiguous,
    /// Source and destination identify the same pane.
    SelfTarget,
}

impl std::fmt::Display for PaneKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unknown => "pane key is missing",
            Self::Ambiguous => "pane key is duplicated",
            Self::SelfTarget => "pane cannot target itself",
        })
    }
}

impl std::error::Error for PaneKeyError {}

impl<T> EnzPaneGridState<T> {
    /// Copy the current application keys, topology and requested split ratios.
    /// Pixel minimums can place rendered seams differently from these ratios.
    #[must_use]
    pub fn snapshot(&self) -> PaneTree<T>
    where
        T: Clone,
    {
        fn copy<T: Clone>(node: &native::Node, state: &native::State<T>) -> PaneTree<T> {
            match node {
                native::Node::Pane(pane) => {
                    PaneTree::Pane(state.get(*pane).expect("pane tree leaf has state").clone())
                }
                native::Node::Split {
                    axis, ratio, a, b, ..
                } => PaneTree::split(
                    match axis {
                        native::Axis::Vertical => Orientation::Vertical,
                        native::Axis::Horizontal => Orientation::Horizontal,
                    },
                    *ratio,
                    copy(a, state),
                    copy(b, state),
                ),
            }
        }
        copy(self.panes.layout(), &self.panes)
    }

    fn topology_changed(&mut self) {
        self.separators = SeparatorModel::from_node(self.panes.layout(), self.tokens);
        self.pointer_resizing.set(false);
        self.focus_visible.set(false);
    }

    pub(super) fn drag_event(&self, event: native::DragEvent) -> PaneDragEvent<T>
    where
        T: Clone,
    {
        let key = |pane| {
            self.panes
                .get(pane)
                .expect("gesture pane has state")
                .clone()
        };
        match event {
            native::DragEvent::Picked { pane } => PaneDragEvent::Picked { key: key(pane) },
            native::DragEvent::Canceled { pane } => PaneDragEvent::Canceled { key: key(pane) },
            native::DragEvent::Dropped { pane, target } => PaneDragEvent::Dropped {
                key: key(pane),
                target: match target {
                    native::Target::Edge(edge) => PaneDropTarget::Edge(PaneEdge::from_native(edge)),
                    native::Target::Pane(pane, region) => PaneDropTarget::Pane {
                        key: key(pane),
                        region: PaneRegion::from_native(region),
                    },
                },
            },
        }
    }
}

impl<T: PartialEq> EnzPaneGridState<T> {
    fn keyed_pane(&self, key: &T) -> Result<native::Pane, PaneKeyError> {
        let mut matches = self.panes.iter().filter(|(_, value)| *value == key);
        let pane = *matches.next().ok_or(PaneKeyError::Unknown)?.0;
        if matches.next().is_some() {
            return Err(PaneKeyError::Ambiguous);
        }
        Ok(pane)
    }

    /// Swap two unique application keys and clear separator focus and resize state.
    ///
    /// # Errors
    /// Missing, duplicated or identical keys leave the state unchanged.
    pub fn swap_panes(&mut self, first: &T, second: &T) -> Result<(), PaneKeyError> {
        let first = self.keyed_pane(first)?;
        let second = self.keyed_pane(second)?;
        if first == second {
            return Err(PaneKeyError::SelfTarget);
        }
        self.panes.swap(first, second);
        self.topology_changed();
        Ok(())
    }

    /// Apply a keyed center swap or edge split and clear separator interaction state.
    /// A split starts at a requested ratio of one half; native pixel minimums apply.
    ///
    /// # Errors
    /// Missing, duplicated or self-targeting pane keys leave the state unchanged.
    pub fn drop_pane(&mut self, key: &T, target: &PaneDropTarget<T>) -> Result<(), PaneKeyError> {
        let source = self.keyed_pane(key)?;
        let target = match target {
            PaneDropTarget::Edge(edge) => native::Target::Edge(edge.native()),
            PaneDropTarget::Pane { key, region } => {
                let destination = self.keyed_pane(key)?;
                if source == destination {
                    return Err(PaneKeyError::SelfTarget);
                }
                native::Target::Pane(destination, region.native())
            }
        };
        self.panes.drop(source, target);
        self.topology_changed();
        Ok(())
    }

    /// Restore unique application keys and topology, discarding widget gestures and focus.
    /// Ratios use the state's existing clamp; widget-local focus is not persisted.
    /// Validation uses pairwise equality, taking quadratic time in the pane count.
    ///
    /// # Errors
    /// Duplicated keys return [`PaneKeyError::Ambiguous`] without changing state.
    pub fn restore(&mut self, tree: PaneTree<T>) -> Result<(), PaneKeyError> {
        fn collect<'a, T>(tree: &'a PaneTree<T>, keys: &mut Vec<&'a T>) {
            match tree {
                PaneTree::Pane(key) => keys.push(key),
                PaneTree::Split { first, second, .. } => {
                    collect(first, keys);
                    collect(second, keys);
                }
            }
        }
        let mut keys = Vec::new();
        collect(&tree, &mut keys);
        for (index, key) in keys.iter().enumerate() {
            if keys[..index].contains(key) {
                return Err(PaneKeyError::Ambiguous);
            }
        }
        self.replace_tree(tree);
        Ok(())
    }
}
