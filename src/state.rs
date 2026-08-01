use crate::{geometry::{Affine, Rect}, raster::{CoverageMask, FillRule}};

#[derive(Clone, Copy, Debug)] pub(crate) enum Clip<'a> {
    None, Rect(Rect), Mask(CoverageMask<'a>),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawState<T, F, S, P> {
    pub(crate) transform: Affine<T>, pub(crate) fill_rule: FillRule,
    pub(crate) flatten: F, pub(crate) stroke: S, pub(crate) paint: P,
    pub(crate) global_alpha: u8,
}

pub(crate) struct GlobalAlphaPaint<'a, S> { sampler: &'a S, alpha: u8 }

impl<'a, S> GlobalAlphaPaint<'a, S> {
    pub(crate) fn new(sampler: &'a S, alpha: u8) -> Self { Self { sampler, alpha } }
}

impl<S: crate::sampler::PaintSampler> crate::sampler::PaintSampler
    for GlobalAlphaPaint<'_, S> {
    fn sample(&self, x: f32, y: f32) -> crate::color::PremulSRGBA8 {
        self.sampler.sample(x, y).scale_alpha(self.alpha)
    }
    fn solid_color(&self) -> Option<crate::color::PremulSRGBA8> {
        self.sampler.solid_color().map(|color| color.scale_alpha(self.alpha))
    }
}

#[cfg(feature = "fixed")] impl<S: crate::fixed::sampler::PaintSampler>
    crate::fixed::sampler::PaintSampler for GlobalAlphaPaint<'_, S> {
    fn sample(&self, x: u32, y: u32) -> crate::color::PremulSRGBA8 {
        self.sampler.sample(x, y).scale_alpha(self.alpha)
    }
    fn solid_color(&self) -> Option<crate::color::PremulSRGBA8> {
        self.sampler.solid_color().map(|color| color.scale_alpha(self.alpha))
    }
}
