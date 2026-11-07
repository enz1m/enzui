//! Renderer-neutral material decisions and sampled-backdrop frame plans.
//!
//! [`record_with`] uses an explicit renderer capability; [`record`] and
//! [`decide`] use the current planning frame's declaration. Explicit capability
//! calls keep their supplied answer even inside a differently declared frame.
//! Callers apply accessibility refusal through [`under_policy`] and motion
//! adjustment through [`MaterialPolicy::settle`]; recording does not read the
//! preference store automatically.
//!
//! [`with_material_frame`] collects ordered regions and content marks. A region
//! samples only earlier marks owned by no material. Its inherited clip restricts
//! composition visibility while full laid-out geometry determines optics and
//! sample padding. Empty plans request no source scope or estimated passes.
//! The renderer must honour the plan's ordering and ownership restrictions.
//!
//! Keep each [`MaterialScope`] inside its recording frame and alive while its
//! foreground paints. Nesting is refused on the thread even without a planning
//! frame; opening a nested frame does not reset that nesting state. Frames
//! restore the previous plan after normal return or unwinding.
//!
//! Geometry and token lengths use logical pixels; the renderer applies its
//! scale when creating physical uniforms. Pass and byte counts describe planned
//! costs, not measurements of actual backend allocations. Optical helpers
//! validate tokens but pass bound dimensions through; callers supply finite
//! geometry. They do not test the rectangle's position or visibility.
//!
//! # Example
//!
//! ```
//! use enzui_core::material::{
//!     MaterialCapability, MaterialGeometry, MaterialRect, MaterialRegionRequest,
//!     with_material_frame,
//! };
//! use enzui_theme::MaterialStyle;
//!
//! let bounds = MaterialRect::new(10.0, 10.0, 160.0, 40.0);
//! let (plan, ()) = with_material_frame(
//!     MaterialCapability::Optical {
//!         max_blur_sigma: 24.0,
//!     },
//!     || {
//!         let request = MaterialRegionRequest {
//!             geometry: MaterialGeometry::new(bounds, [20.0; 4]),
//!             clip: MaterialRect::new(0.0, 0.0, 400.0, 300.0),
//!             tokens: MaterialStyle::Lens.tokens(),
//!             tint: [0.1, 0.1, 0.12],
//!         };
//!         let _scope = enzui_core::material::record(request).expect("optical");
//!     },
//! );
//!
//! assert_eq!(plan.regions().len(), 1);
//! assert!(plan.source_scope().is_some());
//! ```

use enzui_theme::{MaterialPolicy, MaterialTokens};
use std::cell::Cell;
use std::cell::RefCell;

/// Bytes per pixel one filter target costs: the RGBA8 the backend renders in.
pub const SOURCE_BYTES_PER_PIXEL: u64 = 4;

/// Passes a non-empty plan spends resolving and filtering its source once:
/// one resolve into a sampleable target plus two separable filter axes.
pub const SOURCE_PASSES: usize = 3;

/// A logical-pixel rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[must_use]
pub struct MaterialRect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width; a non-positive width makes the rectangle empty.
    pub width: f32,
    /// Height; a non-positive height makes the rectangle empty.
    pub height: f32,
}

impl MaterialRect {
    /// A rectangle at `(x, y)` of `width` by `height`.
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether the rectangle covers no area, which includes any non-finite
    /// extent.
    pub fn is_empty(&self) -> bool {
        !(self.width > 0.0
            && self.height > 0.0
            && self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite())
    }

    /// Right edge.
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Centre point.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// The rectangle grown by `by` on every side.
    pub fn inflated(&self, by: f32) -> Self {
        Self::new(
            self.x - by,
            self.y - by,
            self.width + 2.0 * by,
            self.height + 2.0 * by,
        )
    }

    /// Every coordinate multiplied by `factor`.
    pub fn scaled(&self, factor: f32) -> Self {
        Self::new(
            self.x * factor,
            self.y * factor,
            self.width * factor,
            self.height * factor,
        )
    }

    /// The overlapping area, or `None` when the two do not overlap.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        let candidate = Self::new(x, y, right - x, bottom - y);
        (!candidate.is_empty()).then_some(candidate)
    }

    /// The smallest rectangle covering both.
    pub fn union(&self, other: &Self) -> Self {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// Whether `(x, y)` lies inside the rectangle.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Area in square logical pixels; zero when empty.
    pub fn area(&self) -> f32 {
        if self.is_empty() {
            0.0
        } else {
            self.width * self.height
        }
    }
}

/// The full laid-out shape a material occupies: its lens geometry, regardless
/// of how much of it is currently visible.
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub struct MaterialGeometry {
    /// The shape's bounds in logical pixels.
    pub bounds: MaterialRect,
    /// Corner radii in logical pixels, ordered top-left, top-right,
    /// bottom-left, bottom-right.
    pub radii: [f32; 4],
}

impl MaterialGeometry {
    /// A geometry with each radius sanitized: a non-finite or negative radius
    /// degrades to `0.0`, and every radius is capped to the shape's short half
    /// so the outline stays a well-formed rounded rectangle.
    pub fn new(bounds: MaterialRect, radii: [f32; 4]) -> Self {
        let cap = if bounds.is_empty() {
            0.0
        } else {
            (bounds.width.min(bounds.height)) / 2.0
        };
        let sane = |r: f32| {
            if r.is_finite() && r > 0.0 {
                r.min(cap)
            } else {
                0.0
            }
        };
        Self {
            bounds,
            radii: [
                sane(radii[0]),
                sane(radii[1]),
                sane(radii[2]),
                sane(radii[3]),
            ],
        }
    }

    /// Half the shorter side, or zero for an empty or nonfinite shape.
    pub fn short_half(&self) -> f32 {
        if self.bounds.is_empty() {
            0.0
        } else {
            self.bounds.width.min(self.bounds.height) / 2.0
        }
    }

    /// Largest declared corner radius, capped to the shape's short half.
    pub fn uniform_radius(&self) -> f32 {
        self.radii
            .iter()
            .copied()
            .fold(0.0_f32, f32::max)
            .min(self.short_half())
    }

    /// The geometry in physical pixels for `scale`.
    pub fn scaled(&self, scale: f32) -> Self {
        Self {
            bounds: self.bounds.scaled(scale),
            radii: self.radii.map(|r| r * scale),
        }
    }
}

/// What the renderer owning the current frame can do with a material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaterialCapability {
    /// The renderer can resolve the content beneath a material into a
    /// sampleable target and run the optical pass, filtering at most
    /// `max_blur_sigma` logical pixels of blur.
    Optical {
        /// Largest blur sigma the renderer will honour, in logical pixels.
        max_blur_sigma: f32,
    },
    /// The renderer cannot, and every material must paint its opaque
    /// fallback. Never report a successful filter here.
    Fallback(FallbackReason),
}

/// Why a material paints its opaque fallback instead of a sampled backdrop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// No frame owner declared a material capability, so no renderer is
    /// prepared to sample a backdrop.
    NoMaterialFrame,
    /// The active renderer has no sampled-backdrop path.
    RendererUnsupported,
    /// The surface format cannot be sampled or filtered as required.
    FormatUnsupported,
    /// The material sits inside another material; glass is not filtered
    /// recursively.
    NestedMaterial,
    /// The platform asked for reduced transparency.
    ReducedTransparency,
    /// The platform asked for increased contrast.
    IncreasedContrast,
    /// The style itself is opaque and requests no backdrop.
    OpaqueStyle,
    /// Nothing of the region survives its inherited clip.
    NotVisible,
}

/// Whether a material filters a sampled backdrop, or paints opaque.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaterialDecision {
    /// Filter using tokens whose blur sigma was clamped to the capability.
    /// Other fields retain the caller's values.
    Optical(MaterialTokens),
    /// Paint the opaque fallback for this reason.
    Opaque(FallbackReason),
}

impl MaterialDecision {
    /// Whether a sampled backdrop will be filtered.
    pub fn is_optical(&self) -> bool {
        matches!(self, Self::Optical(_))
    }

    /// The fallback reason, when this decision is opaque.
    pub fn fallback_reason(&self) -> Option<FallbackReason> {
        match self {
            Self::Optical(_) => None,
            Self::Opaque(reason) => Some(*reason),
        }
    }
}

/// Identity of one recorded material region within a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaterialRegionId(u64);

impl MaterialRegionId {
    /// Shared sentinel for regions recorded without an open planning frame.
    /// It is not a collected region ordinal.
    pub const UNPLANNED: Self = Self(u64::MAX);

    /// The frame-local ordinal, increasing in paint order.
    pub fn ordinal(&self) -> u64 {
        self.0
    }
}

/// What a widget states about one material it is painting.
#[derive(Debug, Clone, Copy)]
pub struct MaterialRegionRequest {
    /// The full laid-out shape, independent of clipping.
    pub geometry: MaterialGeometry,
    /// The clip inherited from ancestors, in the same coordinate space.
    pub clip: MaterialRect,
    /// The style's validated tokens.
    pub tokens: MaterialTokens,
    /// The palette colour the material tints toward, as the red, green and
    /// blue channels of the theme's own surface colour. The backend converts
    /// it at the framework's gamma boundary rather than assuming a space.
    pub tint: [f32; 3],
}

/// One material region the renderer must composite.
#[derive(Debug, Clone, Copy)]
pub struct MaterialRegion {
    /// Frame-local identity, increasing in paint order.
    pub id: MaterialRegionId,
    /// The full lens geometry.
    pub geometry: MaterialGeometry,
    /// The part of [`MaterialRegion::geometry`] that survives the inherited clip. Only
    /// this area is composited; the optics still use the full geometry.
    pub visible: MaterialRect,
    /// The tokens this region filters with.
    pub tokens: MaterialTokens,
    /// The palette colour this region tints toward.
    pub tint: [f32; 3],
    /// Content marked at or after this sequence is above the material and is
    /// never part of its source.
    pub source_cutoff: u64,
}

impl MaterialRegion {
    /// The renderer-facing material this region must be composited with.
    ///
    /// Carries the tokens *after* the frame's capability clamped them, and the
    /// opaque colour an unsupported backend paints instead.
    pub fn material(&self) -> iced_core::material::Material {
        self.tokens.to_material(
            self.geometry.radii,
            iced_core::Color::from_rgb(self.tint[0], self.tint[1], self.tint[2]),
        )
    }

    /// The padded source area this region must be able to sample: its full
    /// geometry grown by [`sample_padding`].
    pub fn sample_bounds(&self) -> MaterialRect {
        self.geometry
            .bounds
            .inflated(sample_padding(&self.tokens, &self.geometry))
    }
}

/// One painted content batch, in paint order.
///
/// A renderer integration marks each batch it records so the plan can decide
/// which batches a region may sample.
#[derive(Debug, Clone, Copy)]
pub struct ContentMark {
    /// Paint-order sequence.
    pub sequence: u64,
    /// Where the batch painted.
    pub bounds: MaterialRect,
    /// The material whose foreground this batch belongs to, if any.
    pub owner: Option<MaterialRegionId>,
}

/// The padded area every region of a plan samples from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceScope {
    /// Union of every region's padded sample bounds.
    pub bounds: MaterialRect,
    /// The largest per-region padding the union had to cover.
    pub padding: f32,
}

/// The ordered material work one frame asks for.
#[derive(Debug, Clone, Default)]
pub struct MaterialPlan {
    regions: Vec<MaterialRegion>,
    content: Vec<ContentMark>,
}

impl MaterialPlan {
    /// The regions to composite, in paint order.
    pub fn regions(&self) -> &[MaterialRegion] {
        &self.regions
    }

    /// Every marked content batch, in paint order.
    pub fn content(&self) -> &[ContentMark] {
        &self.content
    }

    /// Whether no material was recorded.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Estimate passes: zero for an empty plan, otherwise three source passes
    /// plus one composite per region and two extra passes if any region has
    /// positive colour diffusion.
    pub fn pass_count(&self) -> usize {
        if self.regions.is_empty() {
            return 0;
        }
        let diffusion = usize::from(
            self.regions
                .iter()
                .any(|region| region.tokens.color_diffusion > 0.0),
        );
        SOURCE_PASSES + self.regions.len() + 2 * diffusion
    }

    /// The area the renderer must resolve into a sampleable target, or `None`
    /// when no material was recorded and nothing may be allocated.
    pub fn source_scope(&self) -> Option<SourceScope> {
        let mut bounds = MaterialRect::default();
        let mut padding = 0.0_f32;
        for region in &self.regions {
            bounds = bounds.union(&region.sample_bounds());
            padding = padding.max(sample_padding(&region.tokens, &region.geometry));
        }
        (!self.regions.is_empty()).then_some(SourceScope { bounds, padding })
    }

    /// Estimate RGBA8 source/filter storage at `scale`, excluding composites.
    ///
    /// Uses the scaled source-scope area rounded down to a whole pixel, with two
    /// targets or three when any region has positive colour diffusion. Empty
    /// plans cost zero. This does not model backend texture rounding or reuse.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the byte-count multiplication overflows `u64`.
    pub fn estimated_source_bytes(&self, scale: f32) -> u64 {
        let Some(scope) = self.source_scope() else {
            return 0;
        };
        let pixels = scope.bounds.scaled(scale).area().max(0.0) as u64;
        let targets = if self
            .regions
            .iter()
            .any(|region| region.tokens.color_diffusion > 0.0)
        {
            3
        } else {
            2
        };
        pixels * SOURCE_BYTES_PER_PIXEL * targets
    }

    /// Whether `region` may sample `mark`.
    ///
    /// True only for a batch painted before the region's boundary that belongs
    /// to no material: a material never samples its own foreground, a sibling
    /// material's foreground, or anything painted above it.
    pub fn samples(&self, region: &MaterialRegion, mark: &ContentMark) -> bool {
        mark.owner.is_none() && mark.sequence < region.source_cutoff
    }

    /// Every content batch `region` may sample, in paint order.
    pub fn source_marks<'a>(
        &'a self,
        region: &MaterialRegion,
    ) -> impl Iterator<Item = &'a ContentMark> + 'a {
        let cutoff = region.source_cutoff;
        self.content
            .iter()
            .filter(move |mark| mark.owner.is_none() && mark.sequence < cutoff)
    }

    /// Return the last recorded region whose visible rectangle contains `(x, y)`.
    /// Rounded-corner geometry is not tested.
    pub fn topmost_at(&self, x: f32, y: f32) -> Option<&MaterialRegion> {
        self.regions
            .iter()
            .rev()
            .find(|region| region.visible.contains(x, y))
    }
}

/// Renderer bevel width in logical pixels, capped at half the shorter side.
///
/// Uses validated tokens. Corner radii do not further cap the bevel; empty
/// shapes yield zero.
pub fn effective_bevel_width(tokens: &MaterialTokens, geometry: &MaterialGeometry) -> f32 {
    renderer_material(tokens, geometry).effective_bevel_width(logical_size(geometry))
}

/// Renderer optical depth in logical pixels, or zero for an empty shape.
///
/// Uses validated tokens and does not cap depth to the shape's short half.
/// Depth remains independent of [`effective_bevel_width`].
pub fn effective_optical_depth(tokens: &MaterialTokens, geometry: &MaterialGeometry) -> f32 {
    renderer_material(tokens, geometry).effective_optical_depth(logical_size(geometry))
}

/// The height of a glass edge rounded with radius `rim`, `inside` in from the
/// silhouette: a quarter circle, flat from `rim` in.
pub fn rim_height(inside: f32, rim: f32) -> f32 {
    iced_core::material::rim_height(inside, rim)
}

/// Main rim-bend displacement at an inward distance, in logical pixels.
///
/// This excludes centre pull and chromatic fringe, which the shader applies
/// independently. Gain zero, depth zero and index one remove this bend only;
/// they do not all remove every shader sample offset.
pub fn displacement_at(tokens: &MaterialTokens, geometry: &MaterialGeometry, inside: f32) -> f32 {
    renderer_material(tokens, geometry).displacement_at(logical_size(geometry), inside)
}

/// Renderer sample-padding displacement estimate in logical pixels.
///
/// Delegates to [`iced_core::material::Material::max_displacement`], using
/// sampled rim displacement, centre pull and chromatic fringe. At index one,
/// that helper returns fringe alone and omits the shader's remaining centre
/// pull; this is an estimate, not a proven bound on all shader sample offsets.
/// Nonpositive side lengths yield zero.
pub fn max_displacement(tokens: &MaterialTokens, geometry: &MaterialGeometry) -> f32 {
    renderer_material(tokens, geometry).max_displacement(logical_size(geometry))
}

/// Estimate source padding beyond a region, in logical pixels.
///
/// Adds three sigma of the wider filter, [`max_displacement`]'s estimate and
/// one pixel for bilinear sampling. It inherits that estimate's limitations;
/// this helper does not measure shader sampling or guarantee full coverage.
pub fn sample_padding(tokens: &MaterialTokens, geometry: &MaterialGeometry) -> f32 {
    renderer_material(tokens, geometry).sample_padding(logical_size(geometry))
}

fn renderer_material(
    tokens: &MaterialTokens,
    geometry: &MaterialGeometry,
) -> iced_core::material::Material {
    tokens.to_material(geometry.radii, iced_core::Color::TRANSPARENT)
}

fn logical_size(geometry: &MaterialGeometry) -> iced_core::Size {
    iced_core::Size::new(geometry.bounds.width, geometry.bounds.height)
}

#[derive(Debug)]
struct Frame {
    capability: MaterialCapability,
    next_sequence: u64,
    next_region: u64,
    open: Vec<MaterialRegionId>,
    regions: Vec<MaterialRegion>,
    content: Vec<ContentMark>,
}

impl Frame {
    fn new(capability: MaterialCapability) -> Self {
        Self {
            capability,
            next_sequence: 0,
            next_region: 0,
            open: Vec::new(),
            regions: Vec::new(),
            content: Vec::new(),
        }
    }

    fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        sequence
    }

    fn take_region_id(&mut self) -> MaterialRegionId {
        let id = MaterialRegionId(self.next_region);
        self.next_region += 1;
        id
    }
}

thread_local! {
    static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };

    // Nesting is refused on the thread, including outside planning frames.
    static NESTING: Cell<usize> = const { Cell::new(0) };
}

/// Collect a material plan under a declared capability and return the paint result.
///
/// Nested calls replace the active plan until return; the outer plan is restored
/// also during unwinding. Material-nesting state is shared across frame scopes.
/// Keep recorded [`MaterialScope`] guards inside their recording callback so
/// their foreground ownership cannot outlive or cross into a different frame.
pub fn with_material_frame<R>(
    capability: MaterialCapability,
    paint: impl FnOnce() -> R,
) -> (MaterialPlan, R) {
    struct Restore(Option<Frame>);

    impl Drop for Restore {
        fn drop(&mut self) {
            FRAME.with(|frame| *frame.borrow_mut() = self.0.take());
        }
    }

    let _restore = Restore(FRAME.with(|frame| frame.borrow_mut().replace(Frame::new(capability))));
    let value = paint();
    let recorded = FRAME.with(|frame| frame.borrow_mut().take());
    let plan = recorded.map_or_else(MaterialPlan::default, |frame| MaterialPlan {
        regions: frame.regions,
        content: frame.content,
    });
    (plan, value)
}

/// Decide using the current frame declaration, without recording a region.
///
/// Outside a frame, nonopaque unnested materials fall back with
/// [`FallbackReason::NoMaterialFrame`]. Nesting and opaque-style refusals take
/// precedence; visibility and accessibility policy are not checked here.
pub fn decide(tokens: &MaterialTokens) -> MaterialDecision {
    decide_with(declared_capability(), tokens)
}

/// Decide using an explicit capability, independently of the frame declaration.
///
/// Checks nesting first, then opaque style, then capability. Optical capability
/// clamps blur sigma to its nonnegative maximum; other token fields are copied
/// unchanged. Callers supply validated tokens and apply [`under_policy`] and
/// [`MaterialPolicy::settle`] separately when needed.
pub fn decide_with(capability: MaterialCapability, tokens: &MaterialTokens) -> MaterialDecision {
    if NESTING.with(Cell::get) > 0 {
        return MaterialDecision::Opaque(FallbackReason::NestedMaterial);
    }

    if tokens.is_opaque() {
        return MaterialDecision::Opaque(FallbackReason::OpaqueStyle);
    }

    match capability {
        MaterialCapability::Fallback(reason) => MaterialDecision::Opaque(reason),
        MaterialCapability::Optical { max_blur_sigma } => {
            MaterialDecision::Optical(MaterialTokens {
                blur_sigma: tokens.blur_sigma.min(max_blur_sigma.max(0.0)),
                ..*tokens
            })
        }
    }
}

pub use enzui_theme::material::{
    MaterialPolicyOwner, PreferenceSet, current_policy, install_policy_owner,
    policy_with_theme_motion, published_layers,
};

/// Apply accessibility refusals to a renderer capability.
///
/// Reduced transparency wins over increased contrast when both are requested.
/// Either replaces even an existing fallback reason. Reduced motion does not
/// change capability; it adjusts tokens through [`MaterialPolicy::settle`].
pub fn under_policy(capability: MaterialCapability, policy: MaterialPolicy) -> MaterialCapability {
    if policy.reduced_transparency {
        return MaterialCapability::Fallback(FallbackReason::ReducedTransparency);
    }

    if policy.increased_contrast {
        return MaterialCapability::Fallback(FallbackReason::IncreasedContrast);
    }

    capability
}

fn declared_capability() -> MaterialCapability {
    frame_capability().unwrap_or(MaterialCapability::Fallback(
        FallbackReason::NoMaterialFrame,
    ))
}

/// The capability the current frame owner declared, if a frame is open.
pub fn frame_capability() -> Option<MaterialCapability> {
    FRAME.with(|frame| frame.borrow().as_ref().map(|frame| frame.capability))
}

/// Record one material region, returning the scope its foreground paints in.
///
/// The returned [`MaterialScope`] must stay alive while the material's own
/// content paints: that is what keeps the foreground out of the backdrop and
/// refuses nested glass. An error means this material must paint its opaque
/// fallback.
///
/// # Errors
///
/// Returns the [`FallbackReason`] when no frame is open, the renderer declined
/// optical material, the style is opaque, the material is nested inside
/// another, or nothing of the region survives its inherited clip.
pub fn record(request: MaterialRegionRequest) -> Result<MaterialScope, FallbackReason> {
    record_with(declared_capability(), request)
}

/// Record a region under explicit capability and open its foreground scope.
///
/// The explicit capability is used even inside a differently declared frame.
/// Without a frame the region is not collected and its ID is
/// [`MaterialRegionId::UNPLANNED`], but the scope still refuses nested material.
/// Within a frame, its clip intersection sets visibility and recording fixes
/// the cutoff excluding later or material-owned content. Keep the scope alive
/// while foreground paints and drop it within the same frame.
///
/// # Errors
///
/// Returns nesting or opaque-style refusal first, then a capability fallback,
/// or [`FallbackReason::NotVisible`] if bounds and clip do not intersect.
/// This does not apply accessibility policy automatically.
pub fn record_with(
    capability: MaterialCapability,
    request: MaterialRegionRequest,
) -> Result<MaterialScope, FallbackReason> {
    let tokens = match decide_with(capability, &request.tokens) {
        MaterialDecision::Optical(tokens) => tokens,
        MaterialDecision::Opaque(reason) => return Err(reason),
    };
    let Some(visible) = request.geometry.bounds.intersection(&request.clip) else {
        return Err(FallbackReason::NotVisible);
    };

    let id = FRAME.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let frame = borrow.as_mut()?;
        let id = frame.take_region_id();
        let source_cutoff = frame.take_sequence();
        frame.regions.push(MaterialRegion {
            id,
            geometry: request.geometry,
            visible,
            tokens,
            tint: request.tint,
            source_cutoff,
        });
        frame.open.push(id);

        Some(id)
    });

    NESTING.with(|nesting| nesting.set(nesting.get() + 1));

    Ok(MaterialScope {
        id: id.unwrap_or(MaterialRegionId::UNPLANNED),
        planned: id.is_some(),
    })
}

/// Mark one painted content batch in paint order.
///
/// A renderer integration calls this for every batch it records so a region
/// can be told which batches lie beneath it. Returns `None` outside a frame
/// scope, where no material is being planned.
pub fn note_content(bounds: MaterialRect) -> Option<ContentMark> {
    FRAME.with(|cell| -> Option<ContentMark> {
        let mut borrow = cell.borrow_mut();
        let frame = borrow.as_mut()?;
        let mark = ContentMark {
            sequence: frame.take_sequence(),
            bounds,
            owner: frame.open.last().copied(),
        };
        frame.content.push(mark);
        Some(mark)
    })
}

/// Foreground-ownership and nesting guard for one recorded material.
///
/// Keep it on the recording thread, alive while the region's content paints,
/// and drop it before leaving the recording frame. In that frame, marked content belongs to the region;
/// while the guard lives, nested recording is refused on the current thread.
#[derive(Debug)]
#[must_use = "the scope must stay alive while the material's own content paints"]
pub struct MaterialScope {
    id: MaterialRegionId,
    planned: bool,
}

impl MaterialScope {
    /// The region this scope belongs to.
    pub fn id(&self) -> MaterialRegionId {
        self.id
    }
}

impl Drop for MaterialScope {
    fn drop(&mut self) {
        NESTING.with(|nesting| nesting.set(nesting.get().saturating_sub(1)));

        if !self.planned {
            return;
        }

        FRAME.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if let Some(frame) = borrow.as_mut()
                && frame.open.last() == Some(&self.id)
            {
                frame.open.pop();
            }
        });
    }
}
