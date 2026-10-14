//! Encoder state machine of a command buffer, shared by frontends and backends.
//!
//! Metal encodes commands into one encoder at a time (render, compute or blit); render-encoder
//! commands (binds, draws, store actions) are only valid inside a render pass, and a bound
//! pipeline must match the pass. These rules are protocol-neutral, so they live here once:
//!
//! * Frontends run every recorded command through an [`EncoderValidator`] at **record time**
//!   (validated against the limits the guest knows), so a violation is reported for the exact
//!   record and poisons the command buffer (gpu.md §9.6).
//! * Backends run the whole submission through a fresh validator at **submit time** (against
//!   the device limits) before executing anything; they never trust the frontend. Device gaps
//!   of the objects themselves (formats, sample counts, vertex input) are checked earlier, when
//!   frontend and backend create them (`caps::texture_support`, `caps::render_pipeline_support`).
//!
//! Errors are [`ModelError::EncoderState`] for command-order violations (reported to the guest
//! as an invalid command) and the usual model errors for invalid descriptors.

use crate::caps::{Capabilities, render_pass_support};
use crate::model::{
    AttachmentDesc, AttachmentSlot, ModelError, PipelineTargets, PrimitiveType, RenderPassDesc,
    RenderTargetLayout, ResourceLimits, StoreAction, TextureDesc,
};

#[derive(Debug, Clone)]
struct RenderState<T> {
    pass: RenderPassDesc<T>,
    layout: RenderTargetLayout,
    pipeline: Option<PipelineTargets>,
    /// Bit `i`: a buffer is bound at vertex buffer index `i` (bindings start empty in every
    /// render encoder).
    vertex_buffers: u32,
    /// Bit `i`: that binding carries an explicit `attributeStride`.
    vertex_strides: u32,
}

#[derive(Debug, Clone)]
enum State<T> {
    Idle,
    Render(Box<RenderState<T>>),
    Compute,
}

/// Tracks the open encoder of one command buffer and checks the order of encoder commands.
///
/// `T` is the texture reference of the render passes (a backend handle in the core).
#[derive(Debug, Clone)]
pub struct EncoderValidator<T> {
    state: State<T>,
}

impl<T> Default for EncoderValidator<T> {
    fn default() -> Self {
        Self { state: State::Idle }
    }
}

fn order(what: &'static str) -> ModelError {
    ModelError::EncoderState(what)
}

impl<T: Copy + PartialEq> EncoderValidator<T> {
    /// No open encoder.
    #[must_use]
    pub const fn new() -> Self {
        Self { state: State::Idle }
    }

    /// Whether no encoder is open.
    #[must_use]
    pub const fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    /// Layout of the open render pass, if any.
    #[must_use]
    pub fn render_layout(&self) -> Option<&RenderTargetLayout> {
        match &self.state {
            State::Render(r) => Some(&r.layout),
            _ => None,
        }
    }

    /// Forgets any open encoder (new command buffer, session reset).
    pub fn reset(&mut self) {
        self.state = State::Idle;
    }

    /// A blit command (`CopyBuffer`): only outside other encoders.
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] inside a render or compute encoder.
    pub fn blit(&self) -> Result<(), ModelError> {
        if self.is_idle() {
            Ok(())
        } else {
            Err(order("blit inside another encoder"))
        }
    }

    /// Opens a render encoder. The pass is validated with `limits` and checked against the
    /// device (`render_pass_support`); `lookup` yields the descriptor of a referenced texture.
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] if an encoder is open; any error of
    /// [`RenderPassDesc::validate`] or `render_pass_support`; whatever `lookup` returns.
    pub fn begin_render<E, F>(
        &mut self,
        pass: &RenderPassDesc<T>,
        limits: &ResourceLimits,
        caps: &Capabilities,
        lookup: F,
    ) -> Result<RenderTargetLayout, E>
    where
        E: From<ModelError>,
        F: FnMut(T) -> Result<TextureDesc, E>,
    {
        if !self.is_idle() {
            return Err(E::from(order("nested encoder")));
        }
        let layout = pass.validate(limits, lookup)?;
        render_pass_support(caps, &layout)?;
        self.state = State::Render(Box::new(RenderState {
            pass: *pass,
            layout,
            pipeline: None,
            vertex_buffers: 0,
            vertex_strides: 0,
        }));
        Ok(layout)
    }

    /// Replaces a deferred store action (`setColorStoreAction:atIndex:`,
    /// `setDepthStoreAction:`, `setStencilStoreAction:`). Metal only allows this for an
    /// attachment whose store action is still `MTLStoreActionUnknown`, and only with a known
    /// action. The pass is validated again with the new action (resolve texture present,
    /// memoryless not stored, device support).
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] outside a render encoder or if the store action is already
    /// known; [`ModelError::InvalidAttachment`] for an empty slot; [`ModelError::InvalidValue`]
    /// for `Unknown`; any error of the re-validation.
    pub fn set_store_action<E, F>(
        &mut self,
        slot: AttachmentSlot,
        action: StoreAction,
        limits: &ResourceLimits,
        caps: &Capabilities,
        lookup: F,
    ) -> Result<(), E>
    where
        E: From<ModelError>,
        F: FnMut(T) -> Result<TextureDesc, E>,
    {
        let State::Render(r) = &mut self.state else {
            return Err(E::from(order("store action outside a render encoder")));
        };
        if action == StoreAction::Unknown {
            return Err(E::from(ModelError::InvalidValue {
                field: "storeAction",
                reason: "MTLStoreActionUnknown cannot be set",
            }));
        }
        let mut pass = r.pass;
        let attachment: Option<&mut AttachmentDesc<T>> = match slot {
            AttachmentSlot::Color(i) => pass
                .color_attachments
                .get_mut(usize::from(i))
                .and_then(Option::as_mut)
                .map(|c| &mut c.attachment),
            AttachmentSlot::Depth => pass.depth_attachment.as_mut().map(|d| &mut d.attachment),
            AttachmentSlot::Stencil => pass.stencil_attachment.as_mut().map(|s| &mut s.attachment),
        };
        let Some(attachment) = attachment else {
            return Err(E::from(ModelError::InvalidAttachment {
                slot,
                reason: "no attachment in this slot",
            }));
        };
        if attachment.store != StoreAction::Unknown {
            return Err(E::from(order(
                "store action already known (only MTLStoreActionUnknown can be replaced)",
            )));
        }
        attachment.store = action;
        let layout = pass.validate(limits, lookup)?;
        render_pass_support(caps, &layout)?;
        r.pass = pass;
        r.layout = layout;
        Ok(())
    }

    /// `setRenderPipelineState:`: the pipeline must match the open pass (formats, sample
    /// count, topology class for layered rendering; Metal checks this when the pipeline is
    /// set).
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] outside a render encoder;
    /// [`PipelineTargets::check_compatible`] errors.
    pub fn set_pipeline(&mut self, pipeline: &PipelineTargets) -> Result<(), ModelError> {
        let State::Render(r) = &mut self.state else {
            return Err(order("render pipeline state set outside a render encoder"));
        };
        pipeline.check_compatible(&r.layout)?;
        r.pipeline = Some(*pipeline);
        Ok(())
    }

    /// `setVertexBuffer:…atIndex:` (`bound` = non-nil buffer, `explicit_stride` = the
    /// `attributeStride:` variant). Indices ≥ 32 are not tracked (the caller rejects them).
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] outside a render encoder.
    pub fn set_vertex_buffer(
        &mut self,
        index: u32,
        bound: bool,
        explicit_stride: bool,
    ) -> Result<(), ModelError> {
        let State::Render(r) = &mut self.state else {
            return Err(order("render command outside a render encoder"));
        };
        let bit = 1u32.checked_shl(index).unwrap_or(0);
        let set = |mask: &mut u32, on: bool| {
            if on {
                *mask |= bit;
            } else {
                *mask &= !bit;
            }
        };
        set(&mut r.vertex_buffers, bound);
        set(&mut r.vertex_strides, bound && explicit_stride);
        Ok(())
    }

    /// Any other render-encoder command (binds of textures, samplers, depth/stencil state).
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] outside a render encoder.
    pub fn render_command(&self) -> Result<(), ModelError> {
        if matches!(self.state, State::Render(_)) {
            Ok(())
        } else {
            Err(order("render command outside a render encoder"))
        }
    }

    /// A draw: needs a render encoder, a bound pipeline, a primitive type of the pipeline's
    /// topology class, and every vertex buffer the pipeline's vertex descriptor fetches from
    /// (with an explicit stride for dynamic-stride layouts).
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] outside a render encoder or without pipeline;
    /// [`ModelError::InvalidValue`] for a primitive type of another topology class.
    pub fn draw(&self, primitive: PrimitiveType) -> Result<(), ModelError> {
        let State::Render(r) = &self.state else {
            return Err(order("draw outside a render encoder"));
        };
        let Some(pipeline) = &r.pipeline else {
            return Err(order("draw without a render pipeline state"));
        };
        if !pipeline.accepts(primitive) {
            return Err(ModelError::InvalidValue {
                field: "primitiveType",
                reason: "not of the pipeline's input primitive topology class",
            });
        }
        if pipeline.vertex_buffers & !r.vertex_buffers != 0 {
            return Err(order(
                "draw with a vertex buffer of the vertex descriptor not bound",
            ));
        }
        if pipeline.dynamic_stride_buffers & !r.vertex_strides != 0 {
            return Err(order(
                "dynamic-stride vertex buffer bound without attributeStride",
            ));
        }
        Ok(())
    }

    /// Closes the render encoder.
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] without a render encoder or if a deferred store action
    /// was never set.
    pub fn end_render(&mut self) -> Result<(), ModelError> {
        match &self.state {
            State::Render(r) if r.layout.deferred_store_action => {
                Err(order("store action still unknown at the end of encoding"))
            }
            State::Render(_) => {
                self.state = State::Idle;
                Ok(())
            }
            _ => Err(order("EndRenderPass without render encoder")),
        }
    }

    /// Opens a compute encoder.
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] if an encoder is open.
    pub fn begin_compute(&mut self) -> Result<(), ModelError> {
        if !self.is_idle() {
            return Err(order("nested encoder"));
        }
        self.state = State::Compute;
        Ok(())
    }

    /// A dispatch with `threads_per_threadgroup` (product 1…`max_threads_per_threadgroup`).
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] outside a compute encoder, [`ModelError::InvalidValue`]
    /// for the thread count.
    pub fn dispatch(
        &self,
        threads_per_threadgroup: [u32; 3],
        limits: &ResourceLimits,
    ) -> Result<(), ModelError> {
        if !matches!(self.state, State::Compute) {
            return Err(order("dispatch outside compute encoder"));
        }
        let total = threads_per_threadgroup
            .iter()
            .try_fold(1u64, |acc, &v| acc.checked_mul(u64::from(v)));
        let max = u64::from(limits.max_threads_per_threadgroup);
        if matches!(total, Some(t) if (1..=max).contains(&t)) {
            Ok(())
        } else {
            Err(ModelError::InvalidValue {
                field: "threadsPerThreadgroup",
                reason: "out of range",
            })
        }
    }

    /// Closes the compute encoder.
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] without a compute encoder.
    pub fn end_compute(&mut self) -> Result<(), ModelError> {
        if !matches!(self.state, State::Compute) {
            return Err(order("EndComputePass without compute encoder"));
        }
        self.state = State::Idle;
        Ok(())
    }

    /// End of the command buffer: no encoder may be open.
    ///
    /// # Errors
    ///
    /// [`ModelError::EncoderState`] if an encoder is still open.
    pub fn finish(&self) -> Result<(), ModelError> {
        if self.is_idle() {
            Ok(())
        } else {
            Err(order("command buffer ends inside an encoder"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ColorAttachmentDesc, DepthAttachmentDesc, LoadAction, MultisampleDepthResolveFilter,
        PixelFormat, PrimitiveTopologyClass, RenderPipelineDesc, ResolveTarget, StorageMode,
        TextureType, TextureUsage,
    };

    const L: ResourceLimits = ResourceLimits::APPLE7;

    fn caps() -> Capabilities {
        Capabilities::rtx4050_assumed()
    }

    /// Textures by index: 0 = 4× MSAA BGRA8 render target, 1 = single-sample BGRA8 render
    /// target (resolve destination), 2 = memoryless 4× MSAA Depth32Float.
    fn tex(i: u8) -> Result<TextureDesc, ModelError> {
        let rt = TextureUsage::RENDER_TARGET;
        let base = TextureDesc::new_2d(PixelFormat::BGRA8Unorm, 64, 64, rt);
        Ok(match i {
            0 => TextureDesc {
                texture_type: TextureType::Type2DMultisample,
                sample_count: 4,
                ..base
            },
            1 => base,
            2 => TextureDesc {
                texture_type: TextureType::Type2DMultisample,
                sample_count: 4,
                pixel_format: PixelFormat::Depth32Float,
                storage_mode: StorageMode::Memoryless,
                ..base
            },
            _ => return Err(ModelError::Unsupported("test texture")),
        })
    }

    fn msaa_pass(store: StoreAction) -> RenderPassDesc<u8> {
        let mut pass = RenderPassDesc::empty();
        let mut color = AttachmentDesc::new(0u8, LoadAction::Clear, store);
        color.resolve = Some(ResolveTarget {
            texture: 1,
            level: 0,
            slice: 0,
            depth_plane: 0,
        });
        pass.color_attachments[0] = Some(ColorAttachmentDesc {
            attachment: color,
            clear_color: Default::default(),
        });
        pass.depth_attachment = Some(DepthAttachmentDesc {
            attachment: AttachmentDesc::new(2, LoadAction::Clear, StoreAction::DontCare),
            clear_depth: 1.0,
            resolve_filter: MultisampleDepthResolveFilter::Sample0,
        });
        pass
    }

    fn pipeline() -> PipelineTargets {
        let mut p = RenderPipelineDesc::default();
        p.color_attachments[0].pixel_format = Some(PixelFormat::BGRA8Unorm);
        p.depth_attachment_pixel_format = Some(PixelFormat::Depth32Float);
        p.raster_sample_count = 4;
        p.input_primitive_topology = PrimitiveTopologyClass::Triangle;
        p.targets()
    }

    #[test]
    fn render_encoder_life_cycle() {
        let mut v = EncoderValidator::new();
        assert!(v.is_idle());
        assert_eq!(
            v.draw(PrimitiveType::Triangle),
            Err(order("draw outside a render encoder"))
        );
        assert!(v.render_command().is_err());
        assert!(v.set_pipeline(&pipeline()).is_err());
        let layout = v
            .begin_render(
                &msaa_pass(StoreAction::MultisampleResolve),
                &L,
                &caps(),
                tex,
            )
            .unwrap();
        assert_eq!(layout.sample_count, 4);
        assert_eq!(v.render_layout(), Some(&layout));
        assert!(v.blit().is_err());
        assert!(v.begin_compute().is_err());
        assert!(
            v.begin_render(&msaa_pass(StoreAction::Store), &L, &caps(), tex)
                .is_err()
        );
        assert_eq!(
            v.draw(PrimitiveType::Triangle),
            Err(order("draw without a render pipeline state"))
        );
        v.set_pipeline(&pipeline()).unwrap();
        v.render_command().unwrap();
        v.draw(PrimitiveType::TriangleStrip).unwrap();
        assert!(matches!(
            v.draw(PrimitiveType::Line),
            Err(ModelError::InvalidValue { .. })
        ));
        assert!(v.finish().is_err());
        v.end_render().unwrap();
        v.finish().unwrap();
        assert!(v.end_render().is_err());
    }

    #[test]
    fn incompatible_pipeline_is_rejected_when_set() {
        let mut v = EncoderValidator::new();
        v.begin_render(
            &msaa_pass(StoreAction::MultisampleResolve),
            &L,
            &caps(),
            tex,
        )
        .unwrap();
        let mut single = pipeline();
        single.sample_count = 1;
        assert!(matches!(
            v.set_pipeline(&single),
            Err(ModelError::InvalidValue { .. })
        ));
        let mut no_depth = pipeline();
        no_depth.depth_format = None;
        assert!(v.set_pipeline(&no_depth).is_err());
    }

    #[test]
    fn deferred_store_action_must_be_set_once_before_the_end() {
        let mut v = EncoderValidator::new();
        v.begin_render(&msaa_pass(StoreAction::Unknown), &L, &caps(), tex)
            .unwrap();
        assert_eq!(
            v.end_render(),
            Err(order("store action still unknown at the end of encoding"))
        );
        let set = |v: &mut EncoderValidator<u8>, slot, action| {
            v.set_store_action::<ModelError, _>(slot, action, &L, &caps(), tex)
        };
        // Unknown cannot be set; empty slots and known actions cannot be replaced.
        assert!(set(&mut v, AttachmentSlot::Color(0), StoreAction::Unknown).is_err());
        assert!(matches!(
            set(&mut v, AttachmentSlot::Color(3), StoreAction::Store),
            Err(ModelError::InvalidAttachment { .. })
        ));
        assert!(matches!(
            set(&mut v, AttachmentSlot::Stencil, StoreAction::Store),
            Err(ModelError::InvalidAttachment { .. })
        ));
        assert_eq!(
            set(&mut v, AttachmentSlot::Depth, StoreAction::Store),
            Err(order(
                "store action already known (only MTLStoreActionUnknown can be replaced)"
            ))
        );
        // CustomSampleDepthStore is valid Metal but unsupported; the state stays deferred.
        assert!(
            set(
                &mut v,
                AttachmentSlot::Color(0),
                StoreAction::CustomSampleDepthStore
            )
            .unwrap_err()
            .is_unsupported()
        );
        assert!(v.end_render().is_err());
        set(
            &mut v,
            AttachmentSlot::Color(0),
            StoreAction::MultisampleResolve,
        )
        .unwrap();
        assert!(set(&mut v, AttachmentSlot::Color(0), StoreAction::Store).is_err());
        v.end_render().unwrap();
    }

    #[test]
    fn deferred_resolve_needs_a_resolve_texture() {
        let mut pass = msaa_pass(StoreAction::Unknown);
        if let Some(c) = &mut pass.color_attachments[0] {
            c.attachment.resolve = None;
        }
        let mut v = EncoderValidator::new();
        v.begin_render(&pass, &L, &caps(), tex).unwrap();
        assert!(matches!(
            v.set_store_action::<ModelError, _>(
                AttachmentSlot::Color(0),
                StoreAction::StoreAndMultisampleResolve,
                &L,
                &caps(),
                tex
            ),
            Err(ModelError::InvalidAttachment { .. })
        ));
        // Storing a memoryless attachment is rejected after the fact too.
        let mut v = EncoderValidator::new();
        let mut pass = msaa_pass(StoreAction::MultisampleResolve);
        if let Some(d) = &mut pass.depth_attachment {
            d.attachment.store = StoreAction::Unknown;
        }
        v.begin_render(&pass, &L, &caps(), tex).unwrap();
        assert!(
            v.set_store_action::<ModelError, _>(
                AttachmentSlot::Depth,
                StoreAction::Store,
                &L,
                &caps(),
                tex
            )
            .is_err()
        );
        v.set_store_action::<ModelError, _>(
            AttachmentSlot::Depth,
            StoreAction::DontCare,
            &L,
            &caps(),
            tex,
        )
        .unwrap();
        v.end_render().unwrap();
    }

    #[test]
    fn draws_need_the_vertex_buffers_of_the_vertex_descriptor() {
        use crate::model::{VertexAttributeDesc, VertexDescriptor, VertexFormat, VertexStride};
        let mut desc = RenderPipelineDesc::default();
        desc.color_attachments[0].pixel_format = Some(PixelFormat::BGRA8Unorm);
        desc.depth_attachment_pixel_format = Some(PixelFormat::Depth32Float);
        desc.raster_sample_count = 4;
        let mut v = VertexDescriptor::default();
        for (slot, buffer) in [(0, 0), (1, 3)] {
            v.attributes[slot] = Some(VertexAttributeDesc {
                format: VertexFormat::Float4,
                offset: 0,
                buffer_index: buffer,
            });
        }
        v.layouts[3].stride = VertexStride::Dynamic;
        desc.vertex_descriptor = Some(v);
        let targets = desc.targets();
        assert_eq!(
            (targets.vertex_buffers, targets.dynamic_stride_buffers),
            (0b1001, 0b1000)
        );

        let mut enc = EncoderValidator::new();
        assert!(
            enc.set_vertex_buffer(0, true, false).is_err(),
            "outside a pass"
        );
        enc.begin_render(
            &msaa_pass(StoreAction::MultisampleResolve),
            &L,
            &caps(),
            tex,
        )
        .unwrap();
        enc.set_pipeline(&targets).unwrap();
        let missing = Err(order(
            "draw with a vertex buffer of the vertex descriptor not bound",
        ));
        assert_eq!(enc.draw(PrimitiveType::Triangle), missing);
        enc.set_vertex_buffer(0, true, false).unwrap();
        enc.set_vertex_buffer(3, true, false).unwrap();
        assert_eq!(
            enc.draw(PrimitiveType::Triangle),
            Err(order(
                "dynamic-stride vertex buffer bound without attributeStride"
            ))
        );
        enc.set_vertex_buffer(3, true, true).unwrap();
        enc.draw(PrimitiveType::Triangle).unwrap();
        // Unbinding (nil) counts as missing again; untracked indices are ignored.
        enc.set_vertex_buffer(0, false, false).unwrap();
        enc.set_vertex_buffer(40, true, true).unwrap();
        assert_eq!(enc.draw(PrimitiveType::Triangle), missing);
    }

    #[test]
    fn compute_and_blit_ordering() {
        let mut v = EncoderValidator::<u8>::new();
        v.blit().unwrap();
        assert!(v.dispatch([1, 1, 1], &L).is_err());
        assert!(v.end_compute().is_err());
        v.begin_compute().unwrap();
        assert!(v.blit().is_err());
        v.dispatch([32, 32, 1], &L).unwrap();
        assert!(v.dispatch([33, 32, 1], &L).is_err());
        assert!(v.dispatch([0, 1, 1], &L).is_err());
        assert!(v.dispatch([u32::MAX, u32::MAX, u32::MAX], &L).is_err());
        assert!(v.draw(PrimitiveType::Point).is_err());
        v.end_compute().unwrap();
        v.begin_compute().unwrap();
        v.reset();
        v.finish().unwrap();
    }
}
