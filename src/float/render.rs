//! Floating-point backend presentation and compatibility compositing helpers.

use crate::{color::PremulSRGBA8, sampler::PaintSampler,
    render::{BYTES_PER_PIXEL, GlobalAlphaPaint, Pixmap, solid_blend_terms}};

impl<S: PaintSampler> PaintSampler for GlobalAlphaPaint<'_, S> {
    fn sample(&self, x: f32, y: f32) -> PremulSRGBA8 {
        self.sampler.sample(x, y).scale_alpha(self.alpha)
    }
    fn solid_color(&self) -> Option<PremulSRGBA8> {
        self.sampler.solid_color().map(|color| color.scale_alpha(self.alpha))
    }
}

impl Pixmap<'_> {
    pub(crate) fn write_encoded_pixel(&mut self, x: u32, y: u32,
        color: PremulSRGBA8) {
        let offset = y as usize * self.stride as usize +
                     x as usize * BYTES_PER_PIXEL as usize;
        self.as_bytes_mut()[offset..offset + BYTES_PER_PIXEL as usize]
            .copy_from_slice(&color.to_array());
    }
}

pub(crate) fn blend_sampled_pixel(pixel: &mut [u8], color: PremulSRGBA8,
    coverage: u8) {
    if coverage == u8::MAX && pixel[3] == 0 {
        pixel.copy_from_slice(&color.to_array());
        return;
    }
    blend_solid_pixel(pixel, solid_blend_terms(color, coverage));
}

fn blend_solid_pixel(pixel: &mut [u8], (source, alpha, inverse): ([u8; 3], u8, u8)) {
    if pixel[3] == 0 {
        pixel.copy_from_slice(&[source[0], source[1], source[2], alpha]);
        return;
    }
    let mul_div_255 = |a, b| (a as u16 * b as u16 + 127).div_euclid(255) as u8;
    for (channel, source) in pixel[..3].iter_mut().zip(source) {
        *channel = source.saturating_add(mul_div_255(*channel, inverse));
    }
    pixel[3] = alpha.saturating_add(mul_div_255(pixel[3], inverse));
}
