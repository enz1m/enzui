//! Lifetime rules for an XDG portal parent lease.
//!
//! A parented portal request carries a `parent_window` handle that is only
//! meaningful while the exported protocol object behind it is alive. Every
//! adopter therefore owes the same four things: hold the lease for exactly as
//! long as the request wants the parent relationship, release it once when the
//! request settles or is cancelled, retire it the moment the surface it names
//! is replaced, and refuse a response that belongs to a lease it no longer
//! holds. [`ParentLease`] is that owner, generic over the lease type so the
//! rules can be exercised without a Wayland connection or a portal transport.
//!
//! # Where the surface identity comes from
//!
//! [`SurfaceIncarnation`] is not minted here. A counter kept beside the
//! request would be a bookkeeping value, not the identity of a protocol object,
//! and could not tell a destroyed surface from a later one that reuses its
//! numeric id. The value must be minted by whoever creates the surface, at the
//! instant it is created, and must never be reused; this type only carries that
//! value and compares it. An application window identity is also not a
//! substitute: it names a window, which outlives any single surface, so it
//! cannot by itself witness a replacement.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

mod handoff;

pub use handoff::Handoff;

/// Identity of one surface incarnation, minted by the surface's owner at
/// creation and never reused.
///
/// Ordering is meaningful only in that a replacement never equals the surface
/// it replaced; callers compare for equality rather than magnitude.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct SurfaceIncarnation(pub u64);

/// Identity of one portal request, so a reply from a superseded request can be
/// told apart from the live one.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct RequestGeneration(pub u64);

/// Why a lease stopped being held.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReleaseCause {
    /// The request it parented reached a terminal outcome.
    Settled,
    /// The request was cancelled by the user or the application.
    Cancelled,
    /// A newer request replaced it.
    Superseded,
    /// The surface it named was replaced by a different incarnation.
    SurfaceReplaced,
    /// The window that owned the surface closed.
    WindowClosed,
}

/// What an arriving portal response may do to the held lease.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResponseVerdict {
    /// The response belongs to the held lease and may be applied.
    Accept,
    /// No lease is held, so there is nothing this response can be applied to.
    NoLease,
    /// The response belongs to a request the holder has already replaced.
    StaleGeneration {
        /// The generation currently held.
        held: RequestGeneration,
        /// The generation the response arrived for.
        arrived: RequestGeneration,
    },
    /// The response belongs to a lease whose surface has since been replaced,
    /// so applying it would parent a surface the window no longer has.
    SurfaceReplaced {
        /// The incarnation the held lease names.
        held: SurfaceIncarnation,
        /// The incarnation the window carries now.
        current: SurfaceIncarnation,
    },
}

impl ResponseVerdict {
    /// Whether the response may be applied.
    #[must_use]
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accept)
    }
}

/// A lease handed back to its owner for destruction, with the reason it was
/// given up.
#[derive(Debug)]
pub struct Released<L> {
    /// The lease, which the caller destroys.
    pub lease: L,
    /// Why it was released.
    pub cause: ReleaseCause,
}

struct Held<L> {
    lease: L,
    surface: SurfaceIncarnation,
    generation: RequestGeneration,
}

/// Holds at most one parent lease, bound to the surface incarnation it was
/// exported for and the request generation it parents.
///
/// Every method that gives the lease up returns it, so the caller performs the
/// destruction and a test can witness that it happens exactly once. A release
/// on an empty holder returns `None`, which makes the operations idempotent.
pub struct ParentLease<L> {
    held: Option<Held<L>>,
}

impl<L> ParentLease<L> {
    /// A holder with no lease.
    #[must_use]
    pub fn new() -> Self {
        Self { held: None }
    }

    /// Binds `lease` to `surface` for request `generation`.
    ///
    /// A lease already held is given back with [`ReleaseCause::Superseded`],
    /// so installing a new parent can never abandon the previous exported
    /// object.
    pub fn install(
        &mut self,
        lease: L,
        surface: SurfaceIncarnation,
        generation: RequestGeneration,
    ) -> Option<Released<L>> {
        let superseded = self.held.take().map(|held| Released {
            lease: held.lease,
            cause: ReleaseCause::Superseded,
        });
        self.held = Some(Held {
            lease,
            surface,
            generation,
        });
        superseded
    }

    /// Whether a lease is currently held.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.held.is_some()
    }

    /// The held lease.
    #[must_use]
    pub fn lease(&self) -> Option<&L> {
        self.held.as_ref().map(|held| &held.lease)
    }

    /// The surface incarnation the held lease names.
    #[must_use]
    pub fn surface(&self) -> Option<SurfaceIncarnation> {
        self.held.as_ref().map(|held| held.surface)
    }

    /// The request generation the held lease parents.
    #[must_use]
    pub fn generation(&self) -> Option<RequestGeneration> {
        self.held.as_ref().map(|held| held.generation)
    }

    /// Judges a response that arrived for request `arrived` while the window
    /// carries surface incarnation `current`.
    ///
    /// A replaced surface is reported even when the generation matches: the
    /// request is the one that was started, but the parent it named is gone.
    #[must_use]
    pub fn verdict(
        &self,
        arrived: RequestGeneration,
        current: SurfaceIncarnation,
    ) -> ResponseVerdict {
        let Some(held) = self.held.as_ref() else {
            return ResponseVerdict::NoLease;
        };
        if held.generation != arrived {
            return ResponseVerdict::StaleGeneration {
                held: held.generation,
                arrived,
            };
        }
        if held.surface != current {
            return ResponseVerdict::SurfaceReplaced {
                held: held.surface,
                current,
            };
        }
        ResponseVerdict::Accept
    }

    /// Gives the lease up because the request it parented settled.
    pub fn settle(&mut self) -> Option<Released<L>> {
        self.release(ReleaseCause::Settled)
    }

    /// Gives the lease up because the request was cancelled.
    pub fn cancel(&mut self) -> Option<Released<L>> {
        self.release(ReleaseCause::Cancelled)
    }

    /// Gives the lease up because the window closed.
    pub fn window_closed(&mut self) -> Option<Released<L>> {
        self.release(ReleaseCause::WindowClosed)
    }

    /// Gives the lease up if it names a surface other than `current`.
    ///
    /// Returns `None` when the held lease still names the live surface, so a
    /// window event that did not replace the surface costs nothing.
    pub fn surface_replaced(&mut self, current: SurfaceIncarnation) -> Option<Released<L>> {
        if self
            .held
            .as_ref()
            .is_some_and(|held| held.surface != current)
        {
            return self.release(ReleaseCause::SurfaceReplaced);
        }
        None
    }

    /// Gives the lease up with an explicit cause.
    pub fn release(&mut self, cause: ReleaseCause) -> Option<Released<L>> {
        self.held.take().map(|held| Released {
            lease: held.lease,
            cause,
        })
    }
}

impl<L> Default for ParentLease<L> {
    fn default() -> Self {
        Self::new()
    }
}
