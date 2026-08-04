
/*! Porter-Duff Compositing Operators & Blending Modes
```text

      Simple alpha compositing: co = Cs x αs + Cb x αb x (1 - αs)
        resultant alpha of the composite: αo = αs + αb x (1 - αs)

        co: the pre-multiplied pixel value after compositing
        Cs: the color value of the source graphic element being composited
        αs: the alpha value of the source graphic element being composited
        Cb: the color value of the backdrop
        αb: the alpha value of the backdrop
        αo: the alpha value of the composite

      Simple alpha compositing using pre-multiplied values: co = cs + cb x (1 - αs)
        cs: the pre-multiplied color value of the source graphic element
        cb: the pre-multiplied color value of the backdrop

      The blending calculations must NOT use pre-multiplied color values:
        Cr = (1 - αb) x Cs + αb x B(Cb, Cs)

        Cr: the   result color
        Cs: the   source color
        Cb: the backdrop color
        αb: the backdrop alpha
         B: the formula that does the blending

      written as non-premultiplied:   αo x Co = αs x Cs + (1 - αs) x αb x Cb
      now substitute the result of blending for Cs:
        αo x Co = αs x ((1 - αb) x Cs + αb x B(Cb, Cs)) + (1 - αs) x αb x Cb

    General Formula for Compositing and Blending:

      Apply the blend in place: Cs = (1 - αb) x Cs + αb x B(Cb, Cs)
      Composite: ao x Co = αs x Fa x Cs + αb x Fb x Cb

        Cs: is the   source color
        Cb: is the backdrop color
        αs: is the   source alpha
        αb: is the backdrop alpha
        B(Cb, Cs): is the mixing/blending function
        Fa: is defined by the Porter-Duff operator in use
        Fb: is defined by the Porter-Duff operator in use

    https://en.wikipedia.org/wiki/Alpha_compositing

    https://en.wikipedia.org/wiki/Blend_modes

    https://www.w3.org/TR/compositing-1
``` */

use crate::{common::color::{PremulRGBA, PremulSRGBA8, RGBA}, float::{floor, sqrt}};

/// Porter-Duff compositing operators and W3C blending modes.
///
/// Color blending is evaluated in the target's working color space using
/// straight RGB; storage before and after the operation remains premultiplied.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] pub enum CompositeMode {
    //  (Alpha) Porter-Duff Compositing Operators:
    /** No regions are enabled. */ Clear,
    /** Only the      source will be present. */ Copy,
    /** Only the destination will be present. */ Dest,
    /** Source is placed over the destination. */ #[default] SrcOver,
    /** The source that overlaps the destination, replaces the destination. */ SrcIn,
    /** Source is placed, where it falls outside of the destination. */ SrcOut,
    /** Source which overlaps the destination, replaces the destination.
        Destination is placed elsewhere. */ SrcAtop,
    /** Destination is placed over the source. */ DstOver,
    /** Destination which overlaps the source,  replaces the source. */ DstIn,
    /** Destination is placed, where it falls outside of the source. */ DstOut,
    /** Destination which overlaps the source replaces the source.
        Source is placed elsewhere. */ DstAtop,
    /** The non-overlapping regions of source and destination are combined. */ XOR,
    /** Display the sum of the source image and destination image.
        It is defined in the Porter-Duff paper as the 'plus' operator. */ Lighter,
    //  Note: Destination is synonymous with backdrop.

    //  (Color) Blending Modes: https://en.wikipedia.org/wiki/Blend_modes
    /** This is the default attribute which specifies no blending. */ Normal,
    /** The source color is multiplied by the destination color and
        replaces the destination. */ Multiply,
    /** Multiplies the complements of the backdrop and source color values,
        then complements the result. */ Screen,
    /** Multiplies or screens the colors, depending on the backdrop color value. */ Overlay,
    /** Selects the  darker of the backdrop and source colors. */ Darken,
    /** Selects the lighter of the backdrop and source colors. */ Lighten,
    /** Brightens the backdrop color to reflect the source color.
        Painting with black produces no change. */ ColorDodge,
    /** Darkens   the backdrop color to reflect the source color.
        Painting with white produces no change. */ ColorBurn,
    /** Multiplies or screens the colors, depending on the   source color value.
        The effect is similar to shining a    harsh spotlight on the backdrop. */ HardLight,
    /** Darkens or lightens the colors, depending on the source color value.
        The effect is similar to shining a diffused spotlight on the backdrop. */ SoftLight,
    /** Subtracts the darker of the two constituent colors from the lighter color. */ Difference,
    /** Produces an effect similar to that of the Difference mode but
        lower in contrast. */ Exclusion,
    /** Creates a color with the hue of the source color and
        the saturation and luminosity of the backdrop color. */ Hue,
    /** Creates a color with the saturation of the source color and
        the hue and luminosity of the backdrop color. */ Saturation,
    /** Creates a color with the hue and saturation of the source color and
        the luminosity of the backdrop color. */ Color,
    /** Creates a color with the luminosity of the source color and
        the hue and saturation of the backdrop color.
        This produces an inverse effect to that of the Color mode. */ Luminosity,

    //  ...
}

type Q15 = i32;
const U8_MAX: u32 = u8::MAX as _;
const FIXED_ONE: Q15 = i16::MAX as _;

fn blend_channel(cb: Q15, cs: Q15, mode: CompositeMode) -> Q15 {
    fn integer_sqrt(value: u32) -> u32 {
        if value < 2 { return value; }
        let mut root = 1_u32 << ((32 - value.leading_zeros()).div_ceil(2));
        loop {
            let next = (root + value / root) / 2;
            if  next >= root { return  root }
                root =  next;
        }
    }

    let round_mul = |left, right| (left * right + FIXED_ONE / 2) / FIXED_ONE;
    let ratio = |num, den| (((num as u32 * FIXED_ONE as u32 +
        den as u32 / 2) / den as u32) as Q15).min(FIXED_ONE);

    let hard_light = |cb, cs| if cs <= FIXED_ONE / 2 { round_mul(2 * cb, cs) } else {
        FIXED_ONE - round_mul(2 * (FIXED_ONE - cb), FIXED_ONE - cs) };
    let soft_light = |cb: Q15, cs: Q15| {
        let curve = if cb <= FIXED_ONE / 4 {
            round_mul(round_mul(16 * cb - 12 * FIXED_ONE, cb) + 4 * FIXED_ONE, cb)
        } else { integer_sqrt(cb as u32 * FIXED_ONE as u32) as _ };

        let value = if cs <= FIXED_ONE / 2 {
            cb - round_mul(round_mul(FIXED_ONE - 2 * cs, cb), FIXED_ONE - cb)
        } else { cb + round_mul(2 * cs - FIXED_ONE, curve - cb) };
            value.clamp(0, FIXED_ONE)
    };  use CompositeMode::*;

    match mode {
        Normal      => cs,
        Multiply    => round_mul(cb, cs),
        Screen      => cb + cs - round_mul(cb, cs),
        Overlay     => hard_light(cs, cb),
        Darken      => cb.min(cs),
        Lighten     => cb.max(cs),
        ColorDodge  => if cs == FIXED_ONE { FIXED_ONE } else { ratio(cb, FIXED_ONE - cs) },
        ColorBurn   => if cs == 0 { 0 } else { FIXED_ONE - ratio(FIXED_ONE - cb, cs) },
        HardLight   => hard_light(cb, cs),
        SoftLight   => soft_light(cb, cs),
        Difference  => (cb - cs).abs(),
        Exclusion   =>  cb + cs - 2 * round_mul(cb, cs),
        _ => unreachable!("non-separable mode passed to blend_channel"),
    }
}

impl PremulSRGBA8 { // XXX: PremulRGBA<u8>
    fn unpremul_rgb(self) -> [Q15; 3] {
        let [r, g, b, alpha] = self.to_array();
        if alpha == 0 { return [0; 3]; }
        let divisor = alpha as u32;
        let reciprocal = (((FIXED_ONE as u32) << 8) + divisor / 2) / divisor;
        [r, g, b].map(|channel| if channel == alpha { FIXED_ONE } else {
            ((channel as u32 * reciprocal + 128) >> 8) as _ })
    }

    fn blend_non_separable(self, drop: Self, mode: CompositeMode) -> [Q15; 3] {
        let (cb, cs) = (drop.unpremul_rgb(), self.unpremul_rgb());

        let lum = |color: [Q15; 3]|
            (299 * color[0] + 587 * color[1] + 114 * color[2]) / 1000;
        let sat = |color: [Q15; 3]| color[0].max(color[1]).max(color[2]) -
                                    color[0].min(color[1]).min(color[2]);

        let set_lum = |mut color: [Q15; 3], target| {
            let delta = target - lum(color);
            for channel in &mut color { *channel += delta; }
            let l = lum(color);

            let (n, x) = (*color.iter().min().unwrap(), *color.iter().max().unwrap());
            if n < 0 { for channel in &mut color {
                *channel = l + (*channel - l) * l / (l - n);
            } }
            if x > FIXED_ONE { for channel in &mut color {
                *channel = l + (*channel - l) * (FIXED_ONE - l) / (x - l);
            } } color
        };
        let set_sat = |mut color: [Q15; 3], target| {
            let mut order = [0, 1, 2];
            order.sort_unstable_by_key(|index| color[*index]);
            let [min, mid, max] = order;
            if  color[max] > color[min] {
                color[mid] = (color[mid] - color[min]) * target /
                             (color[max] - color[min]);
                color[max] = target;
            } else { color[mid] = 0; color[max] = 0; }
                     color[min] = 0; color
        };  use CompositeMode::*;

        match mode {
            Hue         => set_lum(set_sat(cs, sat(cb)), lum(cb)),
            Saturation  => set_lum(set_sat(cb, sat(cs)), lum(cb)),
            Color       => set_lum(cs, lum(cb)),
            Luminosity  => set_lum(cb, lum(cs)),
            _ => unreachable!("separable mode passed to blend_non_separable"),
        }
    }

    /// Integer RGBA8 compositor for the encoded-sRGB compatibility target.
    pub fn composite(self, drop: Self, mode: CompositeMode) -> Self {
        let ([sr, sg, sb, sa], [dr, dg, db, da]) = (self.to_array(), drop.to_array());
        let div_round = |val, div| (val + div / 2) / div;

        if matches!(mode, Clear | Copy | Dest | SrcOver | SrcIn | SrcOut | SrcAtop |
            DstOver | DstIn | DstOut | DstAtop | XOR | Lighter | Normal) {
            let (fa, fb) = match mode {
                SrcOver |
                Normal  => (255, 255 - sa as u32),
                Lighter => (255, 255),  Clear => (0, 0),
                SrcIn   => (da as _, 0), Copy => (255, 0),
                DstIn   => (0, sa as _), Dest => (0, 255),
                SrcOut  => (255 - da as u32, 0),
                DstOver => (255 - da as u32, 255),
                SrcAtop => (da as _, 255 - sa as u32),
                DstAtop => (255 - da as u32, sa as _),
                XOR     => (255 - da as u32, 255 - sa as u32),
                DstOut  => (0, 255 - sa as u32),
                _ => unreachable!(),
            };

            let channel = |src, drop| div_round(fa * src as u32 + fb * drop as u32,
                U8_MAX).min(U8_MAX) as u8;
            return Self::new(channel(sr, dr), channel(sg, dg), channel(sb, db),
                channel(sa, da)).expect("Porter-Duff preserves premultiplied channels");
        }   use CompositeMode::*;

        let blend = if matches!(mode, Hue | Saturation | Color | Luminosity) {
            self.blend_non_separable(drop, mode)
        } else {
            let (src, drop) = (self.unpremul_rgb(), drop.unpremul_rgb());
            core::array::from_fn(|index| blend_channel(drop[index], src[index], mode))
        };

        let alpha = div_round(sa as u32 *  U8_MAX +
                              da as u32 * (U8_MAX - sa as u32), U8_MAX);
        let channel = |src, drop, blend| {
            debug_assert!((0..=FIXED_ONE).contains(&blend));
            // Premultiplied inputs correlate the three terms; their sum plus
            // rounding is at most 2_134_851_967, still within u32.
            let value = src as u32 * (U8_MAX - da as u32) * FIXED_ONE as u32 +
                       drop as u32 * (U8_MAX - sa as u32) * FIXED_ONE as u32 +
                         sa as u32 * da as u32 * blend as u32;
            div_round(value, U8_MAX * FIXED_ONE as u32).min(alpha) as u8
        };
        Self::new(channel(sr, dr, blend[0]), channel(sg, dg, blend[1]),
                  channel(sb, db, blend[2]), alpha as _)
            .expect("W3C compositing preserves premultiplied channels")
    }
}

/** ```
    use ugl_rs::common::color::RGBA;
    let draw = RGBA::<f32>::new(0.3, 0.2, 0.1, 1.0).premul();
    let back = RGBA::<f32>::new(0.2, 0.4, 0.7, 1.0).premul();

    assert_eq!(draw.drop(back), back);
    assert_eq!(draw.copy(back), draw);
    assert_eq!(draw.src_over(back), draw);
    assert_eq!(draw.dst_over(back), back);

    assert_eq!(draw.plus(back).to_array(), [0.5, 0.6, 0.8, 1.0]);
    assert_eq!(draw.clear().to_array(),    [0.0, 0.0, 0.0, 0.0]);
``` */
impl PremulRGBA<f32> {
    /// Composites premultiplied channels in their caller-selected working space.
    ///
    /// Porter-Duff operators consume premultiplied channels directly. Color blend
    /// modes temporarily recover straight RGB for `B(Cb, Cs)`, then apply the W3C
    /// source-over formula. The caller decides whether the channels represent
    /// encoded sRGB or linear light.
    pub fn composite(self, drop: Self, mode: CompositeMode) -> Self {
        use CompositeMode::*; match mode {
            Clear   => self.clear(),
            Copy    => self.copy(drop),
            Dest    => self.drop(drop),
            SrcIn   => self.src_in(drop),
            DstIn   => self.dst_in(drop),
            SrcAtop => self.src_atop(drop),
            DstAtop => self.dst_atop(drop),
            SrcOut  => self.src_out(drop),
            DstOut  => self.dst_out(drop),
            DstOver => self.dst_over(drop),
            SrcOver |
            Normal  => self.src_over(drop),
            Lighter => self.lighter(drop),
            XOR     => self.xor(drop),
            _ => self.blend_src_over(drop, mode),
        }
    }

    /// Applies a W3C color blend in straight RGB, then source-over composites
    /// the result back into premultiplied storage.
    fn blend_src_over(self, drop: Self, mode: CompositeMode) -> Self {
        let (src_channels, drop_channels) = (self.to_array(), drop.to_array());
        let (src_straight, drop_straight) = (self.unpremul(), drop.unpremul());
        let (sa, ba) = (self.alpha(), drop.alpha());

        let blended = match mode {
            Normal     => src_straight.normal(drop_straight),
            Screen     => src_straight.screen(drop_straight),
            Multiply   => src_straight.multiply(drop_straight),
            Overlay    => src_straight.overlay(drop_straight),
            Lighten    => src_straight.lighten(drop_straight),
            Darken     => src_straight.darken(drop_straight),
            ColorDodge => src_straight.dodge(drop_straight),
            ColorBurn  => src_straight.burn(drop_straight),
            HardLight  => src_straight.hard_light(drop_straight),
            SoftLight  => src_straight.soft_light(drop_straight),
            Difference => src_straight.difference(drop_straight),
            Exclusion  => src_straight.exclusion(drop_straight),
            Hue        => src_straight.hue(drop_straight),
            Color      => src_straight.color(drop_straight),
            Saturation => src_straight.saturation(drop_straight),
            Luminosity => src_straight.luminosity(drop_straight),
            _ => unreachable!("Porter-Duff mode passed to blend_src_over"),
        };  use CompositeMode::*;

        let [br, bg, bb, _] = blended.to_array();
        let (blend, alpha) = ([br, bg, bb], sa + ba * (1. - sa));
        let channel = |index|   src_channels[index] * (1. - ba) +
            drop_channels[index] * (1. - sa) + sa * ba * blend[index];
        Self::from((channel(0).clamp(0., alpha), channel(1).clamp(0., alpha),
                    channel(2).clamp(0., alpha), alpha))
    }

    /// (Alpha) Porter-Duff Compositing Operators:
    ///
    /// Composite: co = Fa x cs + Fb x cb, ao = Fa x αs + Fb x αb.
    fn porter_duff(self, dest: Self, fa: f32, fb: f32) -> Self {
        let ([sr, sg, sb, sa], [dr, dg, db, da]) = (self.to_array(), dest.to_array());
        Self::from(((fa * sr + fb * dr).min(1.), (fa * sg + fb * dg).min(1.),
                    (fa * sb + fb * db).min(1.), (fa * sa + fb * da).min(1.)))
    }
    /* Composite: ao x Co = αs x Fa x Cs + αb x Fb x Cb, ao = αs x Fa + αb x Fb;
    // Output pre-multiplied color with alpha from NON-premultiplied source and
    // destination/backdrop  color  and alpha.
    fn porter_duff(self, dest: Self, fa: f32, fb: f32) -> Self {
        let (fa, fb) =  (fa * self.a, fb * dest.a);
        let r = fa * self.r + fb * dest.r;
        let g = fa * self.g + fb * dest.g;
        let b = fa * self.b + fb * dest.b;
        let a = (fa + fb).min(1.);  Self { r, g, b, a }
    } */

    /// No regions are enabled.
    pub fn clear(self) -> Self { Self::zeroed() }
    /// Only the source will be present.
    pub fn  copy(self, dest: Self) -> Self { self.porter_duff(dest, 1., 0.) }
    /// Only the destination will be present.
    pub fn  drop(self, dest: Self) -> Self { self.porter_duff(dest, 0., 1.) }
    /// Display the sum of the source image and destination image.
    pub fn  plus(self, dest: Self) -> Self { self.porter_duff(dest, 1., 1.) }

    /// Source is placed over the destination.
    pub fn src_over(self, dest: Self) -> Self { self.porter_duff(dest, 1., 1. - self.alpha()) }
    /// Destination is placed over the source.
    pub fn dst_over(self, dest: Self) -> Self { self.porter_duff(dest, 1. - dest.alpha(), 1.) }
    /// Source is placed, where it falls outside of the destination.
    pub fn src_out (self, dest: Self) -> Self { self.porter_duff(dest, 1. - dest.alpha(), 0.) }
    /// Destination is placed, where it falls outside of the source.
    pub fn dst_out (self, dest: Self) -> Self { self.porter_duff(dest, 0., 1. - self.alpha()) }
    /// The source that overlaps the destination, replaces the destination.
    pub fn src_in  (self, dest: Self) -> Self { self.porter_duff(dest, dest.alpha(), 0.) }
    /// Destination which overlaps the source, replaces the source.
    pub fn dst_in  (self, dest: Self) -> Self { self.porter_duff(dest, 0., self.alpha()) }
    /// Display the sum of the source image and destination image.
    pub fn lighter (self, dest: Self) -> Self { self.plus(dest) }

    /// Source which overlaps the destination, replaces the destination.
    /// Destination is placed elsewhere.
    pub fn src_atop(self, dest: Self) -> Self {
        self.porter_duff(dest, dest.alpha(), 1. - self.alpha())
    }
    /// Destination which overlaps the source replaces the source. Source is placed elsewhere.
    pub fn dst_atop(self, dest: Self) -> Self {
        self.porter_duff(dest, 1. - dest.alpha(), self.alpha())
    }
    /// The non-overlapping regions of source and destination are combined.
    pub fn xor(self, dest: Self) -> Self {
        self.porter_duff(dest, 1. - dest.alpha(), 1. - self.alpha())
    }
}

impl RGBA<f32> {    #![allow(unused)]
    /// (Color) Blending/Mixing Modes:
    ///
    /// W3C blending first replaces the straight source color in the overlap:
    ///   `Cs' = (1 - αb) × Cs + αb × B(Cb, Cs)`.
    /// Source-over compositing then combines `Cs'` with the backdrop and computes
    /// the output alpha. This helper evaluates only the separable blend function
    /// `B(Cb, Cs)`; therefore alpha does not participate and remains the source alpha.
    fn blend(self, drop: Self, operation: impl Fn(f32, f32) -> f32) -> Self {
        Self::new(operation(drop.r, self.r), operation(drop.g, self.g),
                  operation(drop.b, self.b), self.a)
    }

    /// This is the default attribute which specifies no blending.
    /// The blending formula simply selects the source color.
    pub fn normal(self, _: Self) -> Self { self } // self.blend(drop, |_, cs| cs)

    /// The source color is multiplied by the destination color and replaces the destination.
    /// The resultant color is always at least as dark as either the source or destination color.
    /// Multiplying any color with white preserves the original color.
    /// Multiplying any color with black results in black.
    pub fn multiply(self, drop: Self) -> Self { self.blend(drop, |cb, cs| cb * cs) }

    /// Multiplies the complements of the backdrop and source color values, then complements
    /// the result. The result color is always at least as light as either of the two
    /// constituent colors. Screening any color with white produces white;
    /// screening with black leaves the original color unchanged. The effect is similar to
    /// projecting multiple photographic slides simultaneously onto a single screen.
    pub fn screen(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| cb + cs - cb * cs)    // 1. - (1. - cb) * (1. - cs)
    }

    /// Selects the darker of the backdrop and source colors. The backdrop is replaced with
    /// the source where the source is darker; otherwise, it is left unchanged.
    pub fn darken (self, drop: Self) -> Self { self.blend(drop, f32::min) }

    /// Selects the lighter of the backdrop and source colors. The backdrop is replaced with
    /// the source where the source is lighter; otherwise, it is left unchanged.
    pub fn lighten(self, drop: Self) -> Self { self.blend(drop, f32::max) }

    /// Brightens the backdrop color to reflect the source color.
    /// Painting with black produces no changes.
    pub fn dodge(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| if cs >= 1. { 1. } else { (cb / (1. - cs)).min(1.) })
    }

    /// Darkens the backdrop color to reflect the source color.
    /// Painting with white produces no change.
    pub fn burn(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| if cs <= 0. { 0. } else { 1. - ((1. - cb) / cs).min(1.) })
    }

    /// Overlay is the inverse of the hard-light blend mode.
    pub fn overlay(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| if cb <= 0.5 { 2. * cb * cs }
            else { 1. - 2. * (1. - cb) * (1. - cs) })
    }

    /// Multiplies or screens the colors, depending on the source color value.
    /// The effect is similar to shining a harsh spotlight on the backdrop.
    pub fn hard_light(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| if cs <= 0.5 { 2. * cb * cs }
            else { 1. - 2. * (1. - cb) * (1. - cs) })
    }

    /// Darkens or lightens the colors, depending on the source color value.
    /// The effect is similar to shining a diffused spotlight on the backdrop.
    pub fn soft_light(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| if cs <= 0.5 { cb * (1. - cb) } else {
            (if cb <= 0.25 { ((cb * 16. - 12.) * cb + 4.) * cb  } else { sqrt(cb) }) - cb
        } * (cs * 2. - 1.) + cb)
    }

    /// Subtracts the darker of the two constituent colors from the lighter color.
    /// Painting with white inverts the backdrop color; painting with black produces no change.
    pub fn difference(self, drop: Self) -> Self { self.blend(drop, |cb, cs| (cb - cs).abs()) }

    /// Produces an effect similar to that of the Difference mode but lower in contrast.
    /// Painting with white inverts the backdrop color; painting with black produces no change
    pub fn exclusion(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| cb + cs - 2. * cb * cs)
    }

    /// Creates a color with the hue of the source color and
    /// the saturation and luminosity of the backdrop color.
    pub fn hue(self, drop: Self) -> Self {  // synonymous with chroma?
        self.set_sat(drop.to_sat()).set_lum(drop.to_lum())
    }

    /// Creates a color with the saturation of the source color and the hue and luminosity
    /// of the backdrop color. Painting with this mode in an area of the backdrop that is
    /// a pure gray (no saturation) produces no change.
    pub fn saturation(self, drop: Self) -> Self {
        let lum = drop.to_lum();
        let mut blended = drop.set_sat(self.to_sat()).set_lum(lum);
        blended.a = self.a;     blended
    }

    /// Creates a color with the hue and saturation of the source color and the luminosity
    /// of the backdrop color. This preserves the gray levels of the backdrop and is useful
    /// for coloring monochrome images or tinting color images.
    pub fn color(self, drop: Self) -> Self { self.set_lum(drop.to_lum()) }

    /// Creates a color with the luminosity of the source color and the hue and saturation
    /// of the backdrop color. This produces an inverse effect to that of the Color mode.
    pub fn luminosity(self, drop: Self) -> Self {
        let mut blended = drop.set_lum(self.to_lum());
        blended.a = self.a;     blended
    }

    /// Luma is the weighted average of gamma-corrected R, G, and B, based on their contribution
    /// to perceived lightness, long used as the monochromatic dimension in color TV broadcast.
    fn to_lum(self) -> f32 { 0.299 * self.r + 0.587 * self.g + 0.114 * self.b }

    /// https://en.wikipedia.org/wiki/HSL_and_HSV
    fn to_sat(self) -> f32 { self.r.max(self.g).max(self.b) - self.r.min(self.g).min(self.b) }

    fn set_sat(mut self, sat: f32) -> Self {
        let (mut cmin, mut cmax) = if self.r < self.g {
            (&mut self.r, &mut self.g) } else { (&mut self.g, &mut self.r) };

        let  mut cmid = &mut self.b;
             if *cmid < *cmin {  cmid = cmin; cmin = &mut self.b }
        else if *cmax < *cmid {  cmid = cmax; cmax = &mut self.b }

        if  *cmin <  *cmax {
            *cmid = (*cmid - *cmin) * sat / (*cmax - *cmin);    *cmax = sat
        } else {     *cmid = 0.;    *cmax = 0.; }    *cmin = 0.;    self
    }

    fn set_lum(mut self, lum: f32) -> Self {
        let d = lum - self.to_lum();
        self.r += d; self.g += d; self.b += d;
        let l = self.to_lum();
        let n = self.r.min(self.g).min(self.b);
        let x = self.r.max(self.g).max(self.b);

        if n < 0. {
            let op = |c| l + (((c - l) * l) / (l - n));
            self.r = op(self.r); self.g = op(self.g); self.b = op(self.b);
        }
        if 1. < x {
            let op = |c| l + (((c - l) * (1. - l)) / (x - l));
            self.r = op(self.r); self.g = op(self.g); self.b = op(self.b);
        }   self
    }

    fn to_hsl(self) -> (f32, f32, f32) {
        let (r, g, b) = (self.r, self.g, self.b);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let l = (max + min) / 2.;  let (h, s);

        if max == min { (h, s) = (0., 0.); } else {   let d = max - min;
            s = if l > 0.5 { d / (2. - max - min) } else { d / (max + min) };

            h =    if max == r { ((g - b) / d + if g < b { 6. } else { 0. }) / 6.
            } else if max == g { ((b - r) / d + 2.) / 6.
            } else {             ((r - g) / d + 4.) / 6. };
        }   (h, s, l)
    }

    fn from_hsl(h: f32, s: f32, l: f32, a: f32) -> Self {
        fn hue_to_rgb(p: f32, q: f32, t: f32) -> f32 {
            let mut t = t;
            if t < 0. { t += 1.; }
            if t > 1. { t -= 1.; }
            if t < 1. / 6. { return p + (q - p) * 6. * t; }
            if t < 1. / 2. { return q; }
            if t < 2. / 3. { return p + (q - p) * (2. / 3. - t) * 6.; }
            p
        }

        let (r, g, b) = if s == 0. { (l, l, l) } else {
            let q = if l < 0.5 { l * (1. + s) } else { l + s - l * s };
            let p = 2. * l - q;
            (hue_to_rgb(p, q, h + 1. / 3.), hue_to_rgb(p, q, h),
             hue_to_rgb(p, q, h - 1. / 3.))
        };  Self { r, g, b, a }
    }

    fn set_lum_hsl(self, lum: f32) -> Self {
        let (h, s, _) = self.to_hsl();
        Self::from_hsl(h, s, lum, self.a)
    }

    fn set_sat_hsl(self, sat: f32) -> Self {
        let (h, _, l) = self.to_hsl();
        Self::from_hsl(h, sat, l, self.a)
    }

    fn set_hue_hsl(self, hue: f32) -> Self {
        let (_, s, l) = self.to_hsl();
        Self::from_hsl(hue, s, l, self.a)
    }

    fn to_hsv(self) -> (f32, f32, f32) {
        let (r, g, b) = (self.r, self.g, self.b);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let (v, d) = (max, max - min);
        let s = if max == 0. { 0. } else { d / max };
        let h = if   d == 0. { 0.
        } else if max == r {
            let h = (g - b) / d;
            ((h % 6.) + if h < 0. { 6. } else { 0. }) / 6.
        } else if max == g { ((b - r) / d + 2.) / 6.
        } else {             ((r - g) / d + 4.) / 6. };
        (h, s, v)
    }

    fn from_hsv(h: f32, s: f32, v: f32, a: f32) -> Self {
        let h = h % 1.;
        let i = floor(h * 6.);
        let f =  h * 6. - i;
        let p = v * (1. - s);
        let q = v * (1. - f * s);
        let t = v * (1. - (1. - f) * s);

        let (r, g, b) = match i as i32 % 6 {
            0 => (v, t, p), 1 => (q, v, p), 2 => (p, v, t),
            3 => (p, q, v), 4 => (t, p, v), _ => (v, p, q),
        };  Self { r, g, b, a }
    }

    fn set_val_hsv(self, val: f32) -> Self {
        let (h, s, _) = self.to_hsv();
        Self::from_hsv(h, s, val, self.a)
    }

    fn set_sat_hsv(self, sat: f32) -> Self {
        let (h, _, v) = self.to_hsv();
        Self::from_hsv(h, sat, v, self.a)
    }

    fn set_hue_hsv(self, hue: f32) -> Self {
        let (_, s, v) = self.to_hsv();
        Self::from_hsv(hue, s, v, self.a)
    }

    /// Simply divides pixel values of one layer with the other, but it's useful for
    /// brightening photos if the colour is on grey or less.
    /// It is also useful for removing a colour tint from a photo.
    pub fn divide(self, drop: Self) -> Self {     // similar to color dodge
        self.blend(drop, |cb, cs|
            if cs == 0. { 1. } else { (cb / cs).min(1.) })
    }

    /// Simply subtracts pixel values of one layer with the other.
    /// In case of negative values, black is displayed.
    pub fn subtract(self, drop: Self) -> Self {   // synonymous with minus?
        self.blend(drop, |cb, cs| (cb - cs).max(0.))
    }

    /// Sums the value in the two layers and subtracts 1.
    /// Blending with white leaves the image unchanged.
    pub fn linear_burn(self, drop: Self) -> Self {    // same as inverse subtract
        self.blend(drop, |cb, cs| (cb + cs - 1.).max(0.))
    }

    /// simply adds pixel values of one layer with the other.
    /// In case of values above 1 (in the case of RGB), white is displayed.
    pub fn linear_dodge(self, drop: Self) -> Self {   // same as additive/addition?
        self.blend(drop, |cb, cs| (cb + cs).min(1.))
    }

    /// Combines Linear Dodge and Linear Burn (rescaled so that neutral colors become middle gray).
    pub fn linear_light(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs|
            if 0.5 <= cb { (cs + (cb - 0.5) * 2.).min(1.) } else { (cs + cb * 2. - 1.).max(0.) })
    }

    /// Combines Color Dodge and Color Burn (rescaled so that neutral colors become middle gray).
    pub fn vivid_light(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs|
            if 0.5 <= cs { if cs == 1. { 1. } else { (cb / (1. - cs) / 2.).min(1.) }
            } else         if cs == 0. { 0. } else { 1. - ((1. - cb) / cs / 2.).min(1.) })
    }

    pub fn hard_mix(self, drop: Self) -> Self {   // use vivid-light?
        self.blend(drop, |cb, cs| if 1. - cs < cb { 1. } else { 0. })
    }

    pub fn pin_light(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs|
            if 0.5 <= cs { cb.max(2. * (cs - 0.5)) } else { cb.min(2. * cs) })
    }

    pub fn overwrite(self, drop: Self) -> Self { self.blend(drop, |cb, _| cb) }
    //  https://docs.unity3d.com/Packages/com.unity.shadergraph@6.9/manual/Blend-Node.html
    //  https://docs.krita.org/en/reference_manual/blending_modes.html
}

#[cfg(test)] mod tests { use super::*;
    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }

    #[test] fn premultiplied_source_over_can_be_composed_repeatedly() {
        let  src = RGBA::new(1.0, 0.0, 0.0, 0.5).premul();
        let drop = RGBA::new(0.0, 0.0, 1.0, 0.5).premul();
        let composite = src.src_over(drop);
        for (actual, expected) in composite.to_array().into_iter()
            .zip([0.5, 0.0, 0.25, 0.75]) {
            assert_close(actual, expected);
        }
        assert_eq!(composite.src_over(Default::default()), composite);
    }

    #[test] fn color_blends_use_straight_rgb_and_return_premultiplied_output() {
        let (src, drop) = ([0.5, 0.0, 0.0, 0.5], [0.0, 0.0, 0.5, 0.5]);
        let premul = |[r, g, b, a]: [f32; 4]| PremulRGBA::from((r, g, b, a));
        assert_eq!(premul(src).composite(premul(drop), CompositeMode::Multiply)
            .to_array(), [0.25, 0.0, 0.25, 0.75]);
        assert_eq!(premul(src).composite(premul(drop), CompositeMode::Screen)
            .to_array(), [0.5, 0.0, 0.5, 0.75]);
        let (src, drop) =
            (RGBA::new(1.0, 0.0, 0.0, 0.25), RGBA::new(0.0, 0.0, 1.0, 0.75));
        assert_eq!(src.screen(drop), RGBA::new(1.0, 0.0, 1.0, 0.25));
        assert_eq!(src.linear_dodge(drop), RGBA::new(1.0, 0.0, 1.0, 0.25));
        for blended in [src.overlay(drop), src.saturation(drop),
                                           src.luminosity(drop)] {
            assert_eq!(blended.a, src.a);
        }
    }

    #[test] fn every_mode_preserves_finite_premultiplied_channels() {
        let modes = [Clear, Copy, Dest, SrcOver, SrcIn, SrcOut, SrcAtop, DstOver, DstIn,
            DstOut, DstAtop, XOR, Lighter, Normal, Multiply, Screen, Overlay, Darken,
            Lighten, ColorDodge, ColorBurn, HardLight, SoftLight, Difference,
            Exclusion, Hue, Saturation, Color, Luminosity];     use CompositeMode::*;
        for mode in modes {
            let result = PremulRGBA::from((0.31, 0.12, 0.48, 0.6))
                .composite((0.08, 0.35, 0.2, 0.5).into(), mode).to_array();
            assert!(result.into_iter().all(f32::is_finite));
            assert!(result[..3].iter().all(|channel|
                *channel >= 0.0 && *channel <= result[3]));
            assert!((0.0..=1.0).contains(&result[3]));
        }
    }

    #[test] fn integer_compositor_tracks_the_float_reference() {
        let modes = [Clear, Copy, Dest, SrcOver, SrcIn, SrcOut, SrcAtop, DstOver, DstIn,
            DstOut, DstAtop, XOR, Lighter, Normal, Multiply, Screen, Overlay, Darken,
            Lighten, ColorDodge, ColorBurn, HardLight, SoftLight, Difference,
            Exclusion, Hue, Saturation, Color, Luminosity];     use CompositeMode::*;
        let mut state = 0x2c92_7613_5a1d_89e7_u64;
        let mut random = || {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (state >> 32) as u8
        };
        let mut premul = || {
            let alpha = random();
            let channel = |value: u8| (value as u16 * (alpha as u16 + 1) / 256) as u8;
            [channel(random()), channel(random()), channel(random()), alpha]
        };
        let normalize = |value: u8| value as f32 / u8::MAX as f32;
        for mode in modes { for _ in 0..512 {
            let (source, backdrop) = (premul(), premul());
            let integer = PremulSRGBA8::from_array(source).unwrap().composite(
                PremulSRGBA8::from_array(backdrop).unwrap(), mode).to_array();
            let reference = |[r, g, b, a]: [u8; 4]|
                PremulRGBA::from((normalize(r), normalize(g), normalize(b), normalize(a)));
            let expected = reference(source).composite(reference(backdrop), mode).to_array()
                .map(|value| (value * u8::MAX as f32 + 0.5) as u8);
            // Intermediate fixed-point rounding may differ slightly from a
            // single final f32 quantization, especially near Dodge/Burn limits.
            for (actual, reference) in integer.into_iter().zip(expected) {
                assert!(actual.abs_diff(reference) <= 3,
                    "{mode:?}: {source:?} over {backdrop:?}: {integer:?} != {expected:?}");
            }
        } }
    }

    #[test] fn integer_compositor_accepts_the_maximum_accumulator() {
        let white = PremulSRGBA8::new(255, 255, 255, 255).unwrap();
        assert_eq!(white.composite(white, CompositeMode::Multiply), white);
    }

    #[test] fn q15_channel_blends_cover_polynomial_boundaries_without_overflow() {
        let values = [0, 1, FIXED_ONE / 4, FIXED_ONE / 2,
            FIXED_ONE / 2 + 1, FIXED_ONE - 1, FIXED_ONE];
        let modes = [Normal, Multiply, Screen, Overlay, Darken, Lighten,
            ColorDodge, ColorBurn, HardLight, SoftLight, Difference, Exclusion];
        use CompositeMode::*;
        for mode in modes { for cb in values { for cs in values {
            assert!((0..=FIXED_ONE).contains(&blend_channel(cb, cs, mode)));
        } } }
    }

    #[test] fn set_sat_preserves_channel_order_and_sets_range() {
        let color = RGBA::new(0.2, 0.5, 0.8, 1.0).set_sat(0.3);
        assert_close(color.r, 0.0);
        assert_close(color.g, 0.15);
        assert_close(color.b, 0.3);
        assert_close(color.to_sat(), 0.3);
    }

    #[test] fn set_lum_reaches_requested_luminosity() {
        for color in [RGBA::new(1.0, 0.0, 0.0, 1.0), RGBA::new(0.2, 0.5, 0.8, 0.5),
                      RGBA::new(0.4, 0.4, 0.4, 0.0)] {
            let adjusted = color.set_lum(0.5);
            assert_close(adjusted.to_lum(), 0.5);
            assert_close(adjusted.a, color.a);
        }
    }
}
