//! `MTLDepthStencilDescriptor`, `MTLStencilDescriptor`, `MTLCompareFunction`,
//! `MTLStencilOperation`.
//!
//! Raw values: public SDK header `MTLDepthStencil.h`. Defaults: Apple documentation of the
//! descriptor properties (depth compare `always`, depth writes off, no stencil; stencil compare
//! `always`, all operations `keep`, masks all ones).

use super::{ModelError, raw_enum};

raw_enum! {
    /// `MTLCompareFunction` (depth test, stencil test, sampler comparison).
    CompareFunction, UnknownCompareFunction {
        /// Never passes.
        Never = 0,
        /// `new < old`.
        Less = 1,
        /// `new == old`.
        Equal = 2,
        /// `new <= old`.
        LessEqual = 3,
        /// `new > old`.
        Greater = 4,
        /// `new != old`.
        NotEqual = 5,
        /// `new >= old`.
        GreaterEqual = 6,
        /// Always passes.
        Always = 7,
    }
}

raw_enum! {
    /// `MTLStencilOperation`.
    StencilOperation, UnknownStencilOperation {
        /// Keep the value.
        Keep = 0,
        /// Set to zero.
        Zero = 1,
        /// Set to the reference value.
        Replace = 2,
        /// Increment, clamp at the maximum.
        IncrementClamp = 3,
        /// Decrement, clamp at zero.
        DecrementClamp = 4,
        /// Bitwise invert.
        Invert = 5,
        /// Increment with wrap-around.
        IncrementWrap = 6,
        /// Decrement with wrap-around.
        DecrementWrap = 7,
    }
}

/// `MTLStencilDescriptor` (one face).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StencilDesc {
    /// Stencil test.
    pub compare: CompareFunction,
    /// Operation when the stencil test fails.
    pub stencil_failure: StencilOperation,
    /// Operation when the stencil test passes and the depth test fails.
    pub depth_failure: StencilOperation,
    /// Operation when both tests pass.
    pub depth_stencil_pass: StencilOperation,
    /// Read mask (`uint32_t` in the SDK; only the low 8 bits reach an 8-bit stencil buffer).
    pub read_mask: u32,
    /// Write mask (as `read_mask`).
    pub write_mask: u32,
}

impl Default for StencilDesc {
    fn default() -> Self {
        Self {
            compare: CompareFunction::Always,
            stencil_failure: StencilOperation::Keep,
            depth_failure: StencilOperation::Keep,
            depth_stencil_pass: StencilOperation::Keep,
            read_mask: u32::MAX,
            write_mask: u32::MAX,
        }
    }
}

impl StencilDesc {
    /// Builds a face from raw SDK values.
    ///
    /// # Errors
    ///
    /// Unknown compare function or stencil operation.
    pub const fn from_raw(
        compare: u32,
        stencil_failure: u32,
        depth_failure: u32,
        depth_stencil_pass: u32,
        read_mask: u32,
        write_mask: u32,
    ) -> Result<Self, ModelError> {
        macro_rules! t {
            ($e:expr) => {
                match $e {
                    Ok(v) => v,
                    Err(e) => return Err(e),
                }
            };
        }
        Ok(Self {
            compare: t!(CompareFunction::from_raw(compare)),
            stencil_failure: t!(StencilOperation::from_raw(stencil_failure)),
            depth_failure: t!(StencilOperation::from_raw(depth_failure)),
            depth_stencil_pass: t!(StencilOperation::from_raw(depth_stencil_pass)),
            read_mask,
            write_mask,
        })
    }

    /// Whether this face can never change the stencil buffer.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.write_mask & 0xff == 0
            || [
                self.stencil_failure,
                self.depth_failure,
                self.depth_stencil_pass,
            ]
            .iter()
            .all(|op| *op == StencilOperation::Keep)
    }
}

/// `MTLDepthStencilDescriptor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DepthStencilDesc {
    /// Depth test.
    pub depth_compare: CompareFunction,
    /// Depth writes.
    pub depth_write_enabled: bool,
    /// Front-face stencil state; `None` = stencil test disabled for front faces.
    pub front_face_stencil: Option<StencilDesc>,
    /// Back-face stencil state; `None` = stencil test disabled for back faces.
    pub back_face_stencil: Option<StencilDesc>,
}

impl Default for DepthStencilDesc {
    fn default() -> Self {
        Self {
            depth_compare: CompareFunction::Always,
            depth_write_enabled: false,
            front_face_stencil: None,
            back_face_stencil: None,
        }
    }
}

impl DepthStencilDesc {
    /// Validates the descriptor. Every combination of valid enum values is valid Metal; this
    /// exists so that all descriptors share the same validate-before-use shape.
    ///
    /// # Errors
    ///
    /// None at present.
    pub const fn validate(&self) -> Result<(), ModelError> {
        Ok(())
    }

    /// Whether any stencil test is enabled.
    #[must_use]
    pub const fn stencil_enabled(&self) -> bool {
        self.front_face_stencil.is_some() || self.back_face_stencil.is_some()
    }

    /// Whether the state can write the depth or stencil buffer (hazard tracking: attachment
    /// read-only or read-write).
    #[must_use]
    pub fn writes(&self) -> bool {
        self.depth_write_enabled
            || self
                .front_face_stencil
                .iter()
                .chain(self.back_face_stencil.iter())
                .any(|s| !s.is_read_only())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_values_and_round_trip() {
        for (i, c) in CompareFunction::ALL.iter().enumerate() {
            assert_eq!(c.raw() as usize, i);
            assert_eq!(CompareFunction::from_raw(c.raw()), Ok(*c));
        }
        assert_eq!(CompareFunction::ALL.len(), 8);
        assert_eq!(StencilOperation::ALL.len(), 8);
        assert_eq!(
            StencilOperation::from_raw(6),
            Ok(StencilOperation::IncrementWrap)
        );
        assert_eq!(
            CompareFunction::from_raw(8),
            Err(ModelError::UnknownCompareFunction(8))
        );
        assert_eq!(
            StencilOperation::from_raw(8),
            Err(ModelError::UnknownStencilOperation(8))
        );
    }

    #[test]
    fn stencil_from_raw() {
        let s = StencilDesc::from_raw(3, 0, 1, 2, 0xff, 0x0f).unwrap();
        assert_eq!(s.compare, CompareFunction::LessEqual);
        assert_eq!(s.depth_stencil_pass, StencilOperation::Replace);
        assert!(!s.is_read_only());
        assert_eq!(
            StencilDesc::from_raw(9, 0, 0, 0, 0, 0),
            Err(ModelError::UnknownCompareFunction(9))
        );
        assert_eq!(
            StencilDesc::from_raw(0, 0, 0, 99, 0, 0),
            Err(ModelError::UnknownStencilOperation(99))
        );
        // Bits above the 8-bit stencil buffer do not count as writes.
        let masked = StencilDesc {
            write_mask: 0xffff_ff00,
            ..s
        };
        assert!(masked.is_read_only());
    }

    #[test]
    fn depth_stencil_state() {
        let d = DepthStencilDesc::default();
        assert!(d.validate().is_ok());
        assert!(!d.stencil_enabled() && !d.writes());
        let w = DepthStencilDesc {
            depth_compare: CompareFunction::Less,
            depth_write_enabled: true,
            ..d
        };
        assert!(w.writes());
        let st = DepthStencilDesc {
            back_face_stencil: Some(StencilDesc {
                depth_stencil_pass: StencilOperation::IncrementWrap,
                ..StencilDesc::default()
            }),
            ..d
        };
        assert!(st.stencil_enabled() && st.writes());
        let ro = DepthStencilDesc {
            front_face_stencil: Some(StencilDesc::default()),
            ..d
        };
        assert!(ro.stencil_enabled() && !ro.writes());
    }
}
