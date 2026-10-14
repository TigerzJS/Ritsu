//! `MTLRenderPassDescriptor`: colour/depth/stencil attachments, multisample resolve, clear
//! values and layered rendering.
//!
//! Raw values: public SDK header `MTLRenderPass.h` (`MTLLoadAction`, `MTLStoreAction`,
//! `MTLStoreActionOptions`, `MTLMultisampleDepthResolveFilter`,
//! `MTLMultisampleStencilResolveFilter`). Rules: Apple documentation of the attachment
//! descriptor properties (a resolve store action needs a resolve texture, otherwise Metal
//! ignores it; clear values only matter with `MTLLoadActionClear`; a deferred store action must
//! be set before the encoder ends) and the Metal Feature Set Tables (MSAA depth resolve,
//! stencil resolve, combined store-and-resolve, layered rendering incl. multisample: all at or
//! below Apple7; at most 8 colour attachments, at most 4 samples on Apple7).
//!
//! The descriptor is generic over the texture reference `T` (a backend handle in the core) and
//! is validated with a lookup function that yields the [`TextureDesc`] of each texture, so the
//! rules stay independent of frontends and backends. What a GPU cannot do (e.g. separate depth
//! and stencil textures, which Vulkan dynamic rendering cannot express) is reported in the
//! returned [`RenderTargetLayout`] and decided by the backend.

use core::fmt;

use super::{
    FormatCaps, LoadAction, ModelError, NumericType, PixelFormat, ResourceLimits, StorageMode,
    StoreAction, TextureDesc, TextureType, TextureUsage, raw_enum,
};

/// Maximum number of colour attachments of a render pass (Metal Feature Set Tables: 8 on all
/// Apple families).
pub const MAX_COLOR_ATTACHMENTS: usize = 8;

raw_enum! {
    /// `MTLMultisampleDepthResolveFilter`.
    MultisampleDepthResolveFilter, UnknownDepthResolveFilter {
        /// Sample 0.
        Sample0 = 0,
        /// Minimum over the samples.
        Min = 1,
        /// Maximum over the samples.
        Max = 2,
    }
}

raw_enum! {
    /// `MTLMultisampleStencilResolveFilter`.
    MultisampleStencilResolveFilter, UnknownStencilResolveFilter {
        /// Sample 0.
        Sample0 = 0,
        /// The sample the depth resolve picked.
        DepthResolvedSample = 1,
    }
}

/// `MTLStoreActionOptions` bit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct StoreActionOptions(u32);

impl StoreActionOptions {
    /// No options.
    pub const NONE: Self = Self(0);
    /// Store with the custom sample positions in effect.
    pub const CUSTOM_SAMPLE_POSITIONS: Self = Self(1);

    /// Validates raw option bits.
    ///
    /// # Errors
    ///
    /// [`ModelError::UnknownStoreActionOptions`] for undefined bits.
    pub const fn from_raw(raw: u32) -> Result<Self, ModelError> {
        if raw & !1 != 0 {
            return Err(ModelError::UnknownStoreActionOptions(raw));
        }
        Ok(Self(raw))
    }

    /// Raw bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// Identifies an attachment in error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttachmentSlot {
    /// Colour attachment `n`.
    Color(u8),
    /// Depth attachment.
    Depth,
    /// Stencil attachment.
    Stencil,
}

impl fmt::Display for AttachmentSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Color(i) => write!(f, "colour attachment {i}"),
            Self::Depth => f.write_str("depth attachment"),
            Self::Stencil => f.write_str("stencil attachment"),
        }
    }
}

/// Destination of a multisample resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResolveTarget<T> {
    /// Single-sample texture.
    pub texture: T,
    /// Mip level.
    pub level: u32,
    /// Array slice (cube face index counts as a slice).
    pub slice: u32,
    /// Depth plane (3D textures).
    pub depth_plane: u32,
}

/// Common part of all attachments (`MTLRenderPassAttachmentDescriptor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttachmentDesc<T> {
    /// Texture rendered into.
    pub texture: T,
    /// Mip level.
    pub level: u32,
    /// Array slice (cube face index counts as a slice).
    pub slice: u32,
    /// Depth plane (3D textures).
    pub depth_plane: u32,
    /// Load action.
    pub load: LoadAction,
    /// Store action.
    pub store: StoreAction,
    /// Store action options.
    pub store_options: StoreActionOptions,
    /// Resolve destination (used only with a resolving store action).
    pub resolve: Option<ResolveTarget<T>>,
}

impl<T> AttachmentDesc<T> {
    /// Level 0, slice 0, no resolve.
    #[must_use]
    pub const fn new(texture: T, load: LoadAction, store: StoreAction) -> Self {
        Self {
            texture,
            level: 0,
            slice: 0,
            depth_plane: 0,
            load,
            store,
            store_options: StoreActionOptions::NONE,
            resolve: None,
        }
    }
}

/// `MTLClearColor` (double precision in the SDK).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClearColor {
    /// Red.
    pub red: f64,
    /// Green.
    pub green: f64,
    /// Blue.
    pub blue: f64,
    /// Alpha.
    pub alpha: f64,
}

impl Default for ClearColor {
    /// SDK default: opaque black.
    fn default() -> Self {
        Self {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alpha: 1.0,
        }
    }
}

/// A clear value in the representation the attachment format needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClearValue {
    /// Float, normalized and extended-range formats.
    Float([f32; 4]),
    /// Unsigned integer formats.
    Uint([u32; 4]),
    /// Signed integer formats.
    Sint([i32; 4]),
}

impl ClearColor {
    /// RGBA as an array.
    #[must_use]
    pub const fn to_array(self) -> [f64; 4] {
        [self.red, self.green, self.blue, self.alpha]
    }

    /// Converts to the clear value of `format`. Integer formats take the value rounded toward
    /// zero and saturated to the integer range, NaN becomes 0 [Annahme: Metal converts the
    /// `double` the same way; to be confirmed by an image test].
    #[must_use]
    pub fn for_format(self, format: PixelFormat) -> ClearValue {
        let c = self.to_array();
        match format.numeric_type() {
            // `as` from f64 truncates toward zero, saturates, and maps NaN to 0.
            NumericType::Uint => ClearValue::Uint(c.map(|v| v as u32)),
            NumericType::Sint => ClearValue::Sint(c.map(|v| v as i32)),
            _ => ClearValue::Float(c.map(|v| v as f32)),
        }
    }
}

/// `MTLRenderPassColorAttachmentDescriptor`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorAttachmentDesc<T> {
    /// Texture, actions, resolve.
    pub attachment: AttachmentDesc<T>,
    /// Clear colour (used with [`LoadAction::Clear`]).
    pub clear_color: ClearColor,
}

/// `MTLRenderPassDepthAttachmentDescriptor`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepthAttachmentDesc<T> {
    /// Texture, actions, resolve.
    pub attachment: AttachmentDesc<T>,
    /// Clear depth (SDK default 1.0).
    pub clear_depth: f64,
    /// How depth samples are resolved.
    pub resolve_filter: MultisampleDepthResolveFilter,
}

/// `MTLRenderPassStencilAttachmentDescriptor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StencilAttachmentDesc<T> {
    /// Texture, actions, resolve.
    pub attachment: AttachmentDesc<T>,
    /// Clear stencil (`uint32_t` in the SDK; an 8-bit stencil buffer keeps the low 8 bits).
    pub clear_stencil: u32,
    /// How stencil samples are resolved.
    pub resolve_filter: MultisampleStencilResolveFilter,
}

/// `MTLRenderPassDescriptor` (visibility buffer, sample buffers, rasterization rate maps and
/// tile/imageblock sizes are not modelled yet).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderPassDesc<T> {
    /// Colour attachments by index; `None` = slot unused.
    pub color_attachments: [Option<ColorAttachmentDesc<T>>; MAX_COLOR_ATTACHMENTS],
    /// Depth attachment.
    pub depth_attachment: Option<DepthAttachmentDesc<T>>,
    /// Stencil attachment.
    pub stencil_attachment: Option<StencilAttachmentDesc<T>>,
    /// Render area width; 0 = smallest attachment width.
    pub render_target_width: u32,
    /// Render area height; 0 = smallest attachment height.
    pub render_target_height: u32,
    /// Number of layers for layered rendering; 0 = not layered.
    pub render_target_array_length: u32,
    /// Sample count of a pass without attachments; 0 is treated as 1.
    pub default_raster_sample_count: u32,
}

impl<T> RenderPassDesc<T> {
    /// A pass without attachments (fill in the fields that are needed).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            color_attachments: [None, None, None, None, None, None, None, None],
            depth_attachment: None,
            stencil_attachment: None,
            render_target_width: 0,
            render_target_height: 0,
            render_target_array_length: 0,
            default_raster_sample_count: 0,
        }
    }
}

/// What a validated render pass looks like to pipelines and backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderTargetLayout {
    /// Pixel format per colour slot.
    pub color_formats: [Option<PixelFormat>; MAX_COLOR_ATTACHMENTS],
    /// Depth attachment format.
    pub depth_format: Option<PixelFormat>,
    /// Stencil attachment format.
    pub stencil_format: Option<PixelFormat>,
    /// Samples per pixel (common to all attachments).
    pub sample_count: u32,
    /// Effective render area width.
    pub width: u32,
    /// Effective render area height.
    pub height: u32,
    /// Layer count for layered rendering (0 = not layered).
    pub layers: u32,
    /// Depth and stencil live in two different textures (Metal "separate depth and stencil
    /// attachments"; Vulkan dynamic rendering needs one image view for both).
    pub separate_depth_stencil: bool,
    /// Some attachment uses [`StoreAction::Unknown`]; its store action must be set before the
    /// encoder ends.
    pub deferred_store_action: bool,
    /// Some depth resolve uses `Min`/`Max` (needs `VK_RESOLVE_MODE_MIN/MAX` support).
    pub depth_resolve_min_max: bool,
    /// One combined depth/stencil texture whose depth and stencil aspects resolve into
    /// different resolve textures or subresources. Vulkan dynamic rendering needs one
    /// resolve image view for both (`VUID-VkRenderingInfo-pDepthAttachment-06086`).
    pub ds_resolve_targets_differ: bool,
    /// One combined depth/stencil texture where only one aspect resolves (needs
    /// `independentResolveNone`).
    pub ds_resolve_one_aspect: bool,
    /// One combined depth/stencil texture where both aspects resolve with different modes,
    /// e.g. depth `Min` and stencil sample 0 (needs `independentResolve`).
    pub ds_resolve_modes_differ: bool,
}

/// One bound subresource, for duplicate detection.
struct Bound<T> {
    slot: AttachmentSlot,
    texture: T,
    level: u32,
    first_layer: u32,
    layers: u32,
}

fn attachment_error<E: From<ModelError>>(slot: AttachmentSlot, reason: &'static str) -> E {
    E::from(ModelError::InvalidAttachment { slot, reason })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Color,
    Depth,
    Stencil,
}

/// Checks level/slice/plane of one (main or resolve) subresource and returns its extent and
/// first layer.
fn check_subresource<E: From<ModelError>>(
    slot: AttachmentSlot,
    td: &TextureDesc,
    level: u32,
    slice: u32,
    depth_plane: u32,
    array_length: u32,
) -> Result<(u32, u32, u32), E> {
    if level >= td.mip_level_count {
        return Err(attachment_error(slot, "mip level out of range"));
    }
    let (w, h, d) = td.level_extent(level);
    let layered = array_length > 0;
    let count = array_length.max(1);
    let first = if td.texture_type == TextureType::Type3D {
        if slice != 0 {
            return Err(attachment_error(slot, "slice must be 0 for a 3D texture"));
        }
        match depth_plane.checked_add(count) {
            Some(end) if end <= d => depth_plane,
            _ => return Err(attachment_error(slot, "depth plane range out of range")),
        }
    } else {
        if depth_plane != 0 {
            return Err(attachment_error(
                slot,
                "depth plane must be 0 for a non-3D texture",
            ));
        }
        let layers = td.layer_count().map_err(E::from)?;
        match slice.checked_add(count) {
            Some(end) if end <= layers => slice,
            _ => return Err(attachment_error(slot, "slice range out of range")),
        }
    };
    if layered
        && td.texture_type != TextureType::Type3D
        && !td.texture_type.is_array()
        && !td.texture_type.is_cube()
    {
        return Err(attachment_error(
            slot,
            "layered rendering needs an array, cube or 3D texture",
        ));
    }
    Ok((w, h, first))
}

impl<T: Copy + PartialEq> RenderPassDesc<T> {
    /// Validates the pass. `lookup` yields the descriptor of a referenced texture (and fails
    /// for unknown handles; its error is passed through).
    ///
    /// # Errors
    ///
    /// [`ModelError::InvalidAttachment`] / [`ModelError::InvalidValue`] for guest errors,
    /// [`ModelError::Unsupported`] for valid Metal Ritsu cannot express (custom sample
    /// positions, `DepthResolvedSample` with a min/max depth resolve, clear depth outside
    /// `[0, 1]`), plus whatever `lookup` returns.
    pub fn validate<E, F>(
        &self,
        limits: &ResourceLimits,
        mut lookup: F,
    ) -> Result<RenderTargetLayout, E>
    where
        E: From<ModelError>,
        F: FnMut(T) -> Result<TextureDesc, E>,
    {
        let array_length = self.render_target_array_length;
        if array_length > limits.max_texture_array_layers {
            return Err(E::from(ModelError::InvalidValue {
                field: "renderTargetArrayLength",
                reason: "exceeds the array layer limit",
            }));
        }

        let mut layout = RenderTargetLayout {
            color_formats: [None; MAX_COLOR_ATTACHMENTS],
            depth_format: None,
            stencil_format: None,
            sample_count: 0,
            width: u32::MAX,
            height: u32::MAX,
            layers: array_length,
            separate_depth_stencil: false,
            deferred_store_action: false,
            depth_resolve_min_max: false,
            ds_resolve_targets_differ: false,
            ds_resolve_one_aspect: false,
            ds_resolve_modes_differ: false,
        };
        let mut bound: Vec<Bound<T>> = Vec::with_capacity(2 * MAX_COLOR_ATTACHMENTS + 4);

        let mut attachments: Vec<(AttachmentSlot, Role, &AttachmentDesc<T>)> = Vec::new();
        for (i, c) in self.color_attachments.iter().enumerate() {
            if let Some(c) = c {
                // `i < MAX_COLOR_ATTACHMENTS` = 8, so the cast is lossless.
                let slot = AttachmentSlot::Color(i as u8);
                if i as u64 >= u64::from(limits.max_color_attachments) {
                    return Err(attachment_error(
                        slot,
                        "index exceeds the colour attachment limit",
                    ));
                }
                attachments.push((slot, Role::Color, &c.attachment));
            }
        }
        if let Some(d) = &self.depth_attachment {
            attachments.push((AttachmentSlot::Depth, Role::Depth, &d.attachment));
        }
        if let Some(s) = &self.stencil_attachment {
            attachments.push((AttachmentSlot::Stencil, Role::Stencil, &s.attachment));
        }

        for &(slot, role, a) in &attachments {
            let td = lookup(a.texture)?;
            let fmt = td.pixel_format;
            let caps = fmt.metal_caps();
            if !td.usage.contains(TextureUsage::RENDER_TARGET) {
                return Err(attachment_error(slot, "texture lacks renderTarget usage"));
            }
            let role_ok = match role {
                Role::Color => caps.contains(FormatCaps::COLOR),
                Role::Depth => caps.contains(FormatCaps::DEPTH),
                Role::Stencil => caps.contains(FormatCaps::STENCIL),
            };
            if !role_ok {
                return Err(attachment_error(
                    slot,
                    "pixel format not renderable in this slot",
                ));
            }
            let (w, h, first) =
                check_subresource::<E>(slot, &td, a.level, a.slice, a.depth_plane, array_length)?;
            layout.width = layout.width.min(w);
            layout.height = layout.height.min(h);
            if layout.sample_count == 0 {
                layout.sample_count = td.sample_count;
            } else if layout.sample_count != td.sample_count {
                return Err(attachment_error(
                    slot,
                    "sample count differs from other attachments",
                ));
            }
            match role {
                Role::Color => {
                    if let AttachmentSlot::Color(i) = slot {
                        layout.color_formats[usize::from(i)] = Some(fmt);
                    }
                }
                Role::Depth => layout.depth_format = Some(fmt),
                Role::Stencil => layout.stencil_format = Some(fmt),
            }

            // Actions.
            if td.storage_mode == StorageMode::Memoryless
                && (a.load == LoadAction::Load || a.store.stores())
            {
                return Err(attachment_error(
                    slot,
                    "memoryless attachment cannot be loaded or stored",
                ));
            }
            if a.store_options != StoreActionOptions::NONE {
                return Err(E::from(ModelError::Unsupported(
                    "store action option customSamplePositions",
                )));
            }
            match a.store {
                StoreAction::CustomSampleDepthStore => {
                    return Err(E::from(ModelError::Unsupported(
                        "MTLStoreActionCustomSampleDepthStore",
                    )));
                }
                StoreAction::Unknown => layout.deferred_store_action = true,
                _ => {}
            }

            // The depth and stencil attachments may share one combined texture.
            let shared_ds = role == Role::Stencil
                && self
                    .depth_attachment
                    .as_ref()
                    .is_some_and(|d| d.attachment.texture == a.texture);
            if !shared_ds {
                bound.push(Bound {
                    slot,
                    texture: a.texture,
                    level: a.level,
                    first_layer: first,
                    layers: array_length.max(1),
                });
            }
        }

        if attachments.is_empty() {
            let samples = self.default_raster_sample_count.max(1);
            if !limits.sample_count_ok(samples) {
                return Err(E::from(ModelError::InvalidSampleCount(samples)));
            }
            layout.sample_count = samples;
            let (w, h) = (self.render_target_width, self.render_target_height);
            if w == 0 || h == 0 {
                return Err(E::from(ModelError::InvalidValue {
                    field: "renderTargetWidth/Height",
                    reason: "a pass without attachments needs an explicit render area",
                }));
            }
            if w > limits.max_texture_dimension_2d || h > limits.max_texture_dimension_2d {
                return Err(E::from(ModelError::InvalidValue {
                    field: "renderTargetWidth/Height",
                    reason: "exceeds the 2D texture limit",
                }));
            }
            layout.width = w;
            layout.height = h;
        } else {
            for (requested, have, field) in [
                (
                    self.render_target_width,
                    &mut layout.width,
                    "renderTargetWidth",
                ),
                (
                    self.render_target_height,
                    &mut layout.height,
                    "renderTargetHeight",
                ),
            ] {
                if requested > *have {
                    return Err(E::from(ModelError::InvalidValue {
                        field,
                        reason: "larger than an attachment",
                    }));
                }
                if requested != 0 {
                    *have = requested;
                }
            }
        }

        // Depth/stencil pairing.
        if let (Some(d), Some(s)) = (&self.depth_attachment, &self.stencil_attachment) {
            let (d, s) = (&d.attachment, &s.attachment);
            if d.texture == s.texture {
                if (d.level, d.slice, d.depth_plane) != (s.level, s.slice, s.depth_plane) {
                    return Err(E::from(ModelError::Unsupported(
                        "depth and stencil attachments on different subresources of one texture",
                    )));
                }
            } else {
                layout.separate_depth_stencil = true;
            }
        }

        // Resolves.
        for &(slot, role, a) in &attachments {
            if !a.store.resolves() {
                continue;
            }
            let Some(r) = a.resolve else {
                return Err(attachment_error(
                    slot,
                    "resolve store action without resolve texture",
                ));
            };
            if layout.sample_count <= 1 {
                return Err(attachment_error(
                    slot,
                    "resolve of a single-sample attachment",
                ));
            }
            let src = lookup(a.texture)?;
            let dst = lookup(r.texture)?;
            if !src.pixel_format.metal_caps().contains(FormatCaps::RESOLVE) {
                return Err(attachment_error(slot, "pixel format cannot be resolved"));
            }
            if dst.pixel_format != src.pixel_format {
                return Err(attachment_error(
                    slot,
                    "resolve texture has a different pixel format",
                ));
            }
            if dst.sample_count != 1 {
                return Err(attachment_error(slot, "resolve texture is multisampled"));
            }
            // [Annahme: conservative] Metal requires renderTarget usage on resolve textures as
            // Vulkan does on resolve attachments.
            if !dst.usage.contains(TextureUsage::RENDER_TARGET) {
                return Err(attachment_error(
                    slot,
                    "resolve texture lacks renderTarget usage",
                ));
            }
            if dst.storage_mode == StorageMode::Memoryless {
                return Err(attachment_error(slot, "resolve texture is memoryless"));
            }
            let (w, h, first) =
                check_subresource::<E>(slot, &dst, r.level, r.slice, r.depth_plane, array_length)?;
            if w < layout.width || h < layout.height {
                return Err(attachment_error(
                    slot,
                    "resolve texture smaller than the render area",
                ));
            }
            match role {
                Role::Depth => {
                    if let Some(d) = &self.depth_attachment
                        && d.resolve_filter != MultisampleDepthResolveFilter::Sample0
                    {
                        layout.depth_resolve_min_max = true;
                    }
                }
                Role::Stencil => {
                    let depth_picks = self.depth_attachment.as_ref().is_some_and(|d| {
                        d.attachment.store.resolves()
                            && d.resolve_filter != MultisampleDepthResolveFilter::Sample0
                    });
                    let follows_depth = self.stencil_attachment.as_ref().is_some_and(|s| {
                        s.resolve_filter == MultisampleStencilResolveFilter::DepthResolvedSample
                    });
                    if depth_picks && follows_depth {
                        return Err(E::from(ModelError::Unsupported(
                            "stencil resolve from the depth-resolved sample with min/max depth resolve",
                        )));
                    }
                }
                Role::Color => {}
            }
            // Depth and stencil of one combined texture resolving into the same target count
            // once; any other combination is a separate binding (and flagged below).
            let shared = role == Role::Stencil
                && self.depth_attachment.as_ref().is_some_and(|d| {
                    d.attachment.texture == a.texture
                        && d.attachment.store.resolves()
                        && d.attachment.resolve == Some(r)
                });
            if !shared {
                bound.push(Bound {
                    slot,
                    texture: r.texture,
                    level: r.level,
                    first_layer: first,
                    layers: array_length.max(1),
                });
            }
        }

        // Resolve of one combined depth/stencil texture: what Vulkan can express depends on
        // the target and the per-aspect modes (stencil always resolves sample 0, or the
        // depth-picked sample, which equals sample 0 here because min/max was rejected above).
        if let (Some(d), Some(s)) = (&self.depth_attachment, &self.stencil_attachment)
            && d.attachment.texture == s.attachment.texture
        {
            match (d.attachment.store.resolves(), s.attachment.store.resolves()) {
                (true, true) => {
                    layout.ds_resolve_targets_differ = d.attachment.resolve != s.attachment.resolve;
                    layout.ds_resolve_modes_differ =
                        d.resolve_filter != MultisampleDepthResolveFilter::Sample0;
                }
                (true, false) | (false, true) => layout.ds_resolve_one_aspect = true,
                (false, false) => {}
            }
        }

        // No subresource may be bound twice (attachment or resolve target).
        for (i, a) in bound.iter().enumerate() {
            for b in bound.iter().take(i) {
                let overlap = a.texture == b.texture
                    && a.level == b.level
                    && u64::from(a.first_layer) < u64::from(b.first_layer) + u64::from(b.layers)
                    && u64::from(b.first_layer) < u64::from(a.first_layer) + u64::from(a.layers);
                if overlap {
                    return Err(attachment_error(a.slot, "subresource bound twice"));
                }
            }
        }

        // Clear values.
        if let Some(d) = &self.depth_attachment
            && d.attachment.load == LoadAction::Clear
        {
            if d.clear_depth.is_nan() {
                return Err(E::from(ModelError::InvalidValue {
                    field: "clearDepth",
                    reason: "NaN",
                }));
            }
            if !(0.0..=1.0).contains(&d.clear_depth) {
                return Err(E::from(ModelError::Unsupported(
                    "clear depth outside [0, 1] (needs VK_EXT_depth_range_unrestricted)",
                )));
            }
        }
        Ok(layout)
    }

    /// Whether any attachment still has a deferred ([`StoreAction::Unknown`]) store action.
    #[must_use]
    pub fn has_deferred_store_action(&self) -> bool {
        self.color_attachments
            .iter()
            .flatten()
            .map(|c| &c.attachment)
            .chain(self.depth_attachment.iter().map(|d| &d.attachment))
            .chain(self.stencil_attachment.iter().map(|s| &s.attachment))
            .any(|a| a.store == StoreAction::Unknown)
    }
}

#[cfg(test)]
mod tests;
