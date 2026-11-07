//! Reconstructs and validates the accessible heading outline of a collected frame.

use crate::collect::CollectedNode;
use crate::semantics::{HeadingLevel, Role};
use iced_core::Rectangle;

/// Numerator of the maximum share of a frame's announceable text nodes that may be
/// headings before [`OutlineFinding::OverClaimed`] fires.
pub const MAX_HEADING_SHARE_NUMERATOR: usize = 1;

/// Denominator paired with [`MAX_HEADING_SHARE_NUMERATOR`] for the maximum heading share.
pub const MAX_HEADING_SHARE_DENOMINATOR: usize = 2;

/// Below this many announceable text nodes the density rule is not applied.
pub const DENSITY_FLOOR_TEXT_NODES: usize = 12;

/// One heading as it appeared in the collected frame, in document order.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadingEntry {
    /// The `aria-level` the node published, if any. `None` is itself a
    /// finding ([`OutlineFinding::UnleveledHeading`]).
    pub level: Option<usize>,
    /// The rung [`Self::level`] names, when it is one this framework
    /// expresses. `None` for an absent or out-of-ladder level.
    pub rung: Option<HeadingLevel>,
    /// The heading's accessible name.
    pub name: Option<String>,
    /// Traversal depth at collection time.
    pub depth: usize,
    /// Screen-space bounds.
    pub bounds: Rectangle,
}

/// One way an outline is unsound.
#[derive(Debug, Clone, PartialEq)]
pub enum OutlineFinding {
    /// More than one `aria-level` 1 heading in the frame.
    MultiplePageHeadings {
        /// The names of every level-1 heading, in document order.
        names: Vec<Option<String>>,
    },
    /// The outline jumped more than one rung deeper without the intervening
    /// level.
    LevelSkip {
        /// The level of the preceding heading.
        from: usize,
        /// The level of this heading.
        to: usize,
        /// This heading's name.
        name: Option<String>,
    },
    /// A collected heading published no outline level.
    UnleveledHeading {
        /// The heading's name.
        name: Option<String>,
    },
    /// A heading with a blank or absent accessible name.
    UnnamedHeading {
        /// The level it published, if any.
        level: Option<usize>,
    },
    /// Headings are too large a share of the frame's announceable text for
    /// heading navigation to be a skimming mechanism.
    OverClaimed {
        /// How many headings the frame published.
        headings: usize,
        /// Headings plus plain `Role::Label`s.
        text_nodes: usize,
    },
}

impl OutlineFinding {
    /// Renders this finding as a single-line diagnostic.
    pub fn describe(&self) -> String {
        match self {
            Self::MultiplePageHeadings { names } => format!(
                "{} level-1 (Page) headings in one frame: {names:?}. A frame is \
                 one page; a second Page heading makes \"top of document\" \
                 ambiguous. Demote all but one to HeadingLevel::Section.",
                names.len()
            ),
            Self::LevelSkip { from, to, name } => format!(
                "outline skips from level {from} to level {to} at {name:?}. A \
                 screen reader announces the level, so the gap tells the user a \
                 section exists that they missed. Use the next rung down, or \
                 add the missing one."
            ),
            Self::UnleveledHeading { name } => format!(
                "heading {name:?} published no aria-level, so it cannot be \
                 placed in the outline and lists flat in every \"list headings\" \
                 view."
            ),
            Self::UnnamedHeading { level } => format!(
                "a heading at level {level:?} has no accessible name: a \
                 navigation stop that announces nothing, which is worse than \
                 not being a heading at all."
            ),
            Self::OverClaimed {
                headings,
                text_nodes,
            } => format!(
                "{headings} of {text_nodes} announceable text nodes claim to be \
                 headings — a MAJORITY (limit \
                 {MAX_HEADING_SHARE_NUMERATOR}/{MAX_HEADING_SHARE_DENOMINATOR}). \
                 Heading navigation is a way to SKIP prose; past one half there \
                 is more structure than content and the gesture has nothing \
                 left to skip. Headings mark structure, not emphasis — a large \
                 or bold run is not a heading."
            ),
        }
    }
}

/// The outline a collected frame publishes, plus the counts the density rule
/// needs.
#[derive(Debug, Clone, Default)]
pub struct HeadingOutline {
    entries: Vec<HeadingEntry>,
    label_count: usize,
}

impl HeadingOutline {
    /// Reconstruct the outline from a collected frame, in document order.
    pub fn of(nodes: &[CollectedNode]) -> Self {
        let mut entries = Vec::new();
        let mut label_count = 0usize;
        for n in nodes {
            if n.semantics.is_heading() {
                entries.push(HeadingEntry {
                    level: n.semantics.level,
                    rung: n.semantics.level.and_then(HeadingLevel::from_level),
                    name: n.semantics.label.clone(),
                    depth: n.depth,
                    bounds: n.bounds,
                });
            } else if n.semantics.role == Role::Label {
                label_count += 1;
            }
        }
        Self {
            entries,
            label_count,
        }
    }

    /// The headings, in document order.
    pub fn entries(&self) -> &[HeadingEntry] {
        &self.entries
    }

    /// Number of non-heading `Role::Label` nodes in the frame.
    pub fn label_count(&self) -> usize {
        self.label_count
    }

    /// Headings plus labels: the announceable text population the density
    /// rule measures against.
    pub fn text_node_count(&self) -> usize {
        self.entries.len() + self.label_count
    }

    /// Returns every detected outline finding in validation order.
    pub fn findings(&self) -> Vec<OutlineFinding> {
        let mut out = Vec::new();

        let page_names: Vec<Option<String>> = self
            .entries
            .iter()
            .filter(|e| e.level == Some(HeadingLevel::Page.level()))
            .map(|e| e.name.clone())
            .collect();
        if page_names.len() > 1 {
            out.push(OutlineFinding::MultiplePageHeadings { names: page_names });
        }

        let mut previous: Option<usize> = None;
        for e in &self.entries {
            if e.name.as_deref().is_none_or(|n| n.trim().is_empty()) {
                out.push(OutlineFinding::UnnamedHeading { level: e.level });
            }
            match e.level {
                None => out.push(OutlineFinding::UnleveledHeading {
                    name: e.name.clone(),
                }),
                Some(level) => {
                    if let Some(prev) = previous
                        && level > prev + 1
                    {
                        out.push(OutlineFinding::LevelSkip {
                            from: prev,
                            to: level,
                            name: e.name.clone(),
                        });
                    }
                    previous = Some(level);
                }
            }
        }

        let text_nodes = self.text_node_count();
        if text_nodes >= DENSITY_FLOOR_TEXT_NODES
            && self.entries.len() * MAX_HEADING_SHARE_DENOMINATOR
                > text_nodes * MAX_HEADING_SHARE_NUMERATOR
        {
            out.push(OutlineFinding::OverClaimed {
                headings: self.entries.len(),
                text_nodes,
            });
        }

        out
    }

    /// True when [`Self::findings`] is empty.
    pub fn is_sound(&self) -> bool {
        self.findings().is_empty()
    }

    /// Panics with every finding rendered and names `screen` in the diagnostic.
    ///
    /// # Panics
    ///
    /// Panics when the outline has any finding.
    pub fn assert_sound(&self, screen: &str) {
        let findings = self.findings();
        assert!(
            findings.is_empty(),
            "UNSOUND HEADING OUTLINE on `{screen}` ({} heading(s), {} \
             label(s)):\n  - {}",
            self.entries.len(),
            self.label_count,
            findings
                .iter()
                .map(OutlineFinding::describe)
                .collect::<Vec<_>>()
                .join("\n  - ")
        );
    }

    /// Asserts that this outline contains at least `min` headings.
    ///
    /// # Panics
    ///
    /// Panics when fewer than `min` headings were collected.
    pub fn assert_heading_floor(&self, screen: &str, min: usize) {
        assert!(
            self.entries.len() >= min,
            "HEADING FLOOR on `{screen}`: {} heading(s) published, at \
             least {min} expected. An empty outline passes every soundness \
             check, so this floor is what distinguishes \"this screen's \
             structure is correct\" from \"this screen published no structure \
             at all\" (a severed view arm, or headings regressed to anonymous \
             Role::Label).",
            self.entries.len()
        );
    }
}

/// Returns the number of heading nodes whose accessible name exactly matches `needle`.
pub fn heading_count(nodes: &[CollectedNode], needle: &str) -> usize {
    nodes
        .iter()
        .filter(|n| n.is_heading() && n.semantics.label.as_deref() == Some(needle))
        .count()
}

/// Returns the screen-space bounds of the heading named exactly `needle`.
///
/// # Panics
///
/// Panics unless exactly one heading has that accessible name.
pub fn heading_bounds(nodes: &[CollectedNode], needle: &str) -> Rectangle {
    let hits: Vec<&CollectedNode> = nodes
        .iter()
        .filter(|n| n.is_heading() && n.semantics.label.as_deref() == Some(needle))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "heading_bounds: expected exactly one heading named {needle:?} to \
         measure, found {}. A promoted heading keeps `role == Role::Label` \
         and is only matched via `is_heading()` — if this is 0, \
         either the screen published no heading of that name or a stale \
         `Role::Heading` query is still counting zero somewhere upstream.",
        hits.len(),
    );
    hits[0].bounds
}
