//! Non-empty route paths, stable route-position keys, and depth-based navigation transitions.

/// A route identity with its title and optional subtitle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    id: String,
    title: String,
    subtitle: Option<String>,
}

impl Route {
    /// Creates a route with a stable application ID and display title.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
        }
    }

    /// Attach a subtitle.
    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// The application's handle for this destination.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The header title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The header subtitle, if any.
    #[must_use]
    pub fn subtitle_text(&self) -> Option<&str> {
        self.subtitle.as_deref()
    }
}

/// Identity of a route ID at a stack depth.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteKey {
    id: String,
    depth: usize,
}

impl RouteKey {
    /// The route id at this position.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// How deep this position sits; the root is `0`.
    #[must_use]
    pub const fn depth(&self) -> usize {
        self.depth
    }

    /// Formats this position as `id@depth`.
    ///
    /// Splitting at the final `@` is unambiguous because depth is decimal; consumers treat it as a persistent opaque key.
    #[must_use]
    pub fn key_string(&self) -> String {
        format!("{}@{}", self.id, self.depth)
    }
}

/// Navigation direction derived from old and new path depths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// The destination path is deeper than the source path.
    Forward,
    /// The destination path is shallower than the source path.
    Backward,
    /// Same depth. A replacement, or no change at all.
    None,
}

/// A non-empty route stack with the active route last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePath {
    // Constructors and mutators preserve a non-empty route stack.
    stack: Vec<Route>,
}

impl RoutePath {
    /// A path containing only a root route.
    #[must_use]
    pub fn root(route: Route) -> Self {
        Self { stack: vec![route] }
    }

    /// Builds a path, returning `None` for an empty sequence.
    #[must_use]
    pub fn from_routes(routes: Vec<Route>) -> Option<Self> {
        if routes.is_empty() {
            None
        } else {
            Some(Self { stack: routes })
        }
    }

    /// The routes, root first.
    #[must_use]
    pub fn routes(&self) -> &[Route] {
        &self.stack
    }

    /// Returns the stack length, always at least one.
    #[must_use]
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Returns `false`; route paths are non-empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Returns the active route.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: a route path is never empty"
    )]
    #[must_use]
    pub fn active(&self) -> &Route {
        self.stack
            .last()
            .expect("a RoutePath is non-empty by construction")
    }

    /// The active route's depth; the root is `0`.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.stack.len() - 1
    }

    /// The identity of the active position.
    #[must_use]
    pub fn active_key(&self) -> RouteKey {
        RouteKey {
            id: self.active().id().to_owned(),
            depth: self.depth(),
        }
    }

    /// Whether the root route is the active one.
    #[must_use]
    pub fn at_root(&self) -> bool {
        self.stack.len() == 1
    }

    /// Returns whether a route can be popped without removing the root.
    #[must_use]
    pub fn can_pop(&self) -> bool {
        !self.at_root()
    }

    /// Push a route on top.
    pub fn push(&mut self, route: Route) {
        self.stack.push(route);
    }

    /// Removes the active route, or returns `None` at the root.
    pub fn pop(&mut self) -> Option<Route> {
        if self.at_root() {
            None
        } else {
            self.stack.pop()
        }
    }

    /// Removes everything above root and reports whether the path changed.
    pub fn pop_to_root(&mut self) -> bool {
        if self.at_root() {
            false
        } else {
            self.stack.truncate(1);
            true
        }
    }

    /// Replaces the active route without changing depth.
    pub fn replace_active(&mut self, route: Route) -> Route {
        let depth = self.depth();
        std::mem::replace(&mut self.stack[depth], route)
    }

    /// Derives direction by comparing this path’s depth with another.
    #[must_use]
    pub fn transition_to(&self, other: &Self) -> Transition {
        match other.depth().cmp(&self.depth()) {
            std::cmp::Ordering::Greater => Transition::Forward,
            std::cmp::Ordering::Less => Transition::Backward,
            std::cmp::Ordering::Equal => Transition::None,
        }
    }
}
