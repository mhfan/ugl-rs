//! Stroke expansion options and scalar reference implementation.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineCap { #[default] Butt, Round, Square, }

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineJoin { #[default] Miter, Round, Bevel, }

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum StrokeError {
    NonFiniteWidth, NonPositiveWidth, NonFiniteMiterLimit, MiterLimitTooSmall,
}

/// Validated device-space stroke parameters.
#[derive(Clone, Copy, Debug, PartialEq)] pub struct StrokeOptions {
    width: f32, miter_limit: f32, cap: LineCap, join: LineJoin,
}

impl StrokeOptions {
    pub fn new(width: f32) -> Result<Self, StrokeError> {
        if !width.is_finite() { return Err(StrokeError::NonFiniteWidth); }
        if  width <= 0.0 { return Err(StrokeError::NonPositiveWidth); }
        Ok(Self { width, ..Self::default() })
    }

    pub fn with_cap(mut self, cap: LineCap) -> Self { self.cap = cap; self }
    pub fn with_join(mut self, join: LineJoin) -> Self { self.join = join; self }

    pub fn with_miter_limit(mut self, miter_limit: f32) -> Result<Self, StrokeError> {
        if !miter_limit.is_finite() { return Err(StrokeError::NonFiniteMiterLimit); }
        if miter_limit < 1.0 { return Err(StrokeError::MiterLimitTooSmall); }
        self.miter_limit = miter_limit;   Ok(self)
    }

    pub fn width(&self) -> f32 { self.width }
    pub fn half_width(&self) -> f32 { self.width * 0.5 }
    pub fn miter_limit(&self) -> f32 { self.miter_limit }
    pub fn cap(&self) -> LineCap { self.cap }
    pub fn join(&self) -> LineJoin { self.join }
}

impl Default for StrokeOptions {
    fn default() -> Self {
        Self { width: 1.0, miter_limit: 4.0, cap: LineCap::Butt, join: LineJoin::Miter }
    }
}

#[cfg(test)] mod tests { use super::*;
    #[test] fn stroke_options_reject_invalid_geometric_states() {
        assert_eq!(StrokeOptions::new(0.0), Err(StrokeError::NonPositiveWidth));
        assert_eq!(StrokeOptions::new(f32::INFINITY), Err(StrokeError::NonFiniteWidth));
        assert_eq!(StrokeOptions::new(2.0).unwrap().with_miter_limit(0.5),
                   Err(StrokeError::MiterLimitTooSmall));
        assert_eq!(StrokeOptions::new(2.0).unwrap().with_miter_limit(f32::NAN),
                   Err(StrokeError::NonFiniteMiterLimit));
    }

    #[test] fn stroke_options_use_device_space_defaults_and_builders() {
        let options = StrokeOptions::new(6.0).unwrap()
            .with_cap(LineCap::Round).with_join(LineJoin::Bevel)
            .with_miter_limit(8.0).unwrap();
        assert_eq!((options.width(), options.half_width(),
                    options.miter_limit()), (6.0, 3.0, 8.0));
        assert_eq!((options.cap(), options.join()), (LineCap::Round, LineJoin::Bevel));
    }
}
