
/** Porter-Duff Compositing Operators & Blending Modes

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

    https://www.w3.org/TR/compositing-1 */

#[derive(Clone, Copy, Debug, PartialEq)] pub enum BlendMode {
    //  (Alpha) Porter-Duff Compositing Operators:
    /** No regions are enabled. */ Clear,
    /** Only the      source will be present. */ Copy,
    /** Only the destination will be present. */ Dest,
    /** Source is placed over the destination. */ SrcOver,
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

pub type CompOp = BlendMode;
pub use crate::color::RGBA;

/* impl RGBA<u8> {
    /// Composite: ao x Co = αs x Fa x Cs + αb x Fb x Cb, ao = αs x Fa + αb x Fb;
    pub fn composite(self, dest: Self, fa: u8, fb: u8) -> Self {
        let (fa, fb) =  ((fa as u32 * self.a as u32 + 128) >> 8,
                                   (fb as u32 * dest.a as u32 + 128) >> 8);
        let a = (fa + fb).min(255);     // XXX: make it non-premultiplied?
        let r = (fa * self.r as u32 + fb * dest.r as u32 + 128) >> 8; // / a;
        let g = (fa * self.g as u32 + fb * dest.g as u32 + 128) >> 8; // / a;
        let b = (fa * self.b as u32 + fb * dest.b as u32 + 128) >> 8; // / a;
        Self { r: r as _, g: g as _, b: b as _, a: a as _ }
    }

    /// Apply the blend in place: Cs = (1 - αb) x Cs + αb x B(Cb, Cs)
    pub fn blend(self, drop: Self, bop: impl Fn(u8, u8) -> u8) -> Self {
        //lerp(self.r, bop(drop.r, self.r), drop.a);    // lerp for g, b as well
        let (inv_a, da) = ((255 - drop.a) as u32, drop.a as u32);
        let r = (inv_a * self.r as u32 + da * bop(drop.r, self.r) as u32 + 128) >> 8;
        let g = (inv_a * self.g as u32 + da * bop(drop.g, self.g) as u32 + 128) >> 8;
        let b = (inv_a * self.b as u32 + da * bop(drop.b, self.b) as u32 + 128) >> 8;
        Self { r: r as _, g: g as _, b: b as _, a: self.a }
    }
} */

/** ```
    use ugl_rs::blend::RGBA;
    let draw = RGBA::new(0.3, 0.2, 0.1, 1.0);
    let back = RGBA::new(0.2, 0.4, 0.7, 1.0);

    assert_eq!(draw.drop(back), back);
    assert_eq!(draw.copy(back), draw);
    assert_eq!(draw.src_over(back), draw);
    assert_eq!(draw.dst_over(back), back);

    // XXX: ...
    assert_eq!(draw.plus(back), (0.5, 0.6, 0.8, 1.0).into());
    assert_eq!(draw.clear(),    (0.0, 0.0, 0.0, 0.0).into());
``` */
impl RGBA<f32> {
    /// (Alpha) Porter-Duff Compositing Operators:
    ///
    /// Composite: ao x Co = αs x Fa x Cs + αb x Fb x Cb, ao = αs x Fa + αb x Fb;
    /// Output pre-multiplied color with alpha from NON-premultiplied source and
    /// destination/backdrop  color  and alpha.
    fn composite(self, dest: Self, fa: f32, fb: f32) -> Self {
        let (fa, fb) =  (fa * self.a, fb * dest.a);
        let r = fa * self.r + fb * dest.r;
        let g = fa * self.g + fb * dest.g;
        let b = fa * self.b + fb * dest.b;
        let a = (fa + fb).min(1.);  Self { r, g, b, a }
        //Self { r: r / a, g: g / a, b: b / a, a }  // XXX: make it non-premultiplied?
    }

    /// No regions are enabled.
    #[inline] pub fn clear(self) -> Self { Self::zeroed() }
    /// Only the source will be present.
    #[inline] pub fn  copy(self, dest: Self) -> Self { self.composite(dest, 1., 0.) }
    /// Only the destination will be present.
    #[inline] pub fn  drop(self, dest: Self) -> Self { self.composite(dest, 0., 1.) }
    /// Display the sum of the source image and destination image.
    #[inline] pub fn  plus(self, dest: Self) -> Self { self.composite(dest, 1., 1.) }

    /// Source is placed over the destination.
    #[inline] pub fn src_over(self, dest: Self) -> Self { self.composite(dest, 1., 1. - self.a) }
    /// Destination is placed over the source.
    #[inline] pub fn dst_over(self, dest: Self) -> Self { self.composite(dest, 1. - dest.a, 1.) }
    /// Source is placed, where it falls outside of the destination.
    #[inline] pub fn src_out (self, dest: Self) -> Self { self.composite(dest, 1. - dest.a, 0.) }
    /// Destination is placed, where it falls outside of the source.
    #[inline] pub fn dst_out (self, dest: Self) -> Self { self.composite(dest, 0., 1. - self.a) }
    /// The source that overlaps the destination, replaces the destination.
    #[inline] pub fn src_in  (self, dest: Self) -> Self { self.composite(dest, dest.a, 0.) }
    /// Destination which overlaps the source, replaces the source.
    #[inline] pub fn dst_in  (self, dest: Self) -> Self { self.composite(dest, 0., self.a) }
    /// Display the sum of the source image and destination image.
    #[inline] pub fn lighter (self, dest: Self) -> Self { self.plus(dest) }

    /// Source which overlaps the destination, replaces the destination.
    /// Destination is placed elsewhere.
    #[inline] pub fn src_atop(self, dest: Self) -> Self {
        self.composite(dest, dest.a, 1. - self.a)
    }
    /// Destination which overlaps the source replaces the source. Source is placed elsewhere.
    #[inline] pub fn dst_atop(self, dest: Self) -> Self {
        self.composite(dest, 1. - dest.a, self.a)
    }
    /// The non-overlapping regions of source and destination are combined.
    #[inline] pub fn xor(self, dest: Self) -> Self {
        self.composite(dest, 1. - dest.a, 1. - self.a)
    }

    /// (Color) Blending/Mixing Modes:
    ///
    /// Apply the blend in place: Cs = (1 - αb) x Cs + αb x B(Cb, Cs)
    fn blend(self, drop: Self, bop: impl Fn(f32, f32) -> f32) -> Self {
        //lerp(self.r, bop(drop.r, self.r), drop.a);    // lerp for g, b as well
        let inv_a = 1. - drop.a;
        let r = inv_a * self.r + drop.a * bop(drop.r, self.r);
        let g = inv_a * self.g + drop.a * bop(drop.g, self.g);
        let b = inv_a * self.b + drop.a * bop(drop.b, self.b);
        Self { r, g, b,  a:  self.a }
    }

    /// This is the default attribute which specifies no blending.
    /// The blending formula simply selects the source color.
    #[inline] pub fn normal(self, _: Self) -> Self { self }

    /// The source color is multiplied by the destination color and replaces the destination.
    /// The resultant color is always at least as dark as either the source or destination color.
    /// Multiplying any color with white preserves the original color.
    /// Multiplying any color with black results in black.
    #[inline] pub fn multiply(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| cb * cs)
    }

    /// Multiplies the complements of the backdrop and source color values, then complements
    /// the result. The result color is always at least as light as either of the two
    /// constituent colors. Screening any color with white produces white;
    /// screening with black leaves the original color unchanged. The effect is similar to
    /// projecting multiple photographic slides simultaneously onto a single screen.
    #[inline] pub fn screen(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| cb + cs - cb * cs)    // 1. - (1. - cb) * (1. - cs)
    }

    /// Selects the darker of the backdrop and source colors. The backdrop is replaced with
    /// the source where the source is darker; otherwise, it is left unchanged.
    #[inline] pub fn darken(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| cb.min(cs))
    }

    /// Selects the lighter of the backdrop and source colors. The backdrop is replaced with
    /// the source where the source is lighter; otherwise, it is left unchanged.
    #[inline] pub fn lighten(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| cb.max(cs))
    }

    /// Brightens the backdrop color to reflect the source color.
    /// Painting with black produces no changes.
    #[inline] pub fn dodge(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs|
            if cs == 1. { 1. } else { (cb / (1. - cs)).min(1.) })
    }

    /// Darkens the backdrop color to reflect the source color.
    /// Painting with white produces no change.
    #[inline] pub fn burn(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs|
            if cs == 0. { 0. } else { 1. - ((1. - cb) / cs).min(1.) })
    }

    /// Overlay is the inverse of the hard-light blend mode.
    #[inline] pub fn overlay(self, drop: Self) -> Self { drop.hard_light(self) }
    /// Multiplies or screens the colors, depending on the source color value.
    /// The effect is similar to shining a harsh spotlight on the backdrop.
    #[inline] pub fn hard_light(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs|
            if cs <= 0.5 { cb * cs * 2. } else { 1. - (1. - cb) * (1. - cs) * 2. })
    }

    /// Darkens or lightens the colors, depending on the source color value.
    /// The effect is similar to shining a diffused spotlight on the backdrop.
    #[inline] pub fn soft_light(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs|
            if cs <= 0.5 { cb * (1. - cb) } else {
                (if cb <= 0.25 { ((cb * 16. - 12.) * cb + 4.) * cb } else { cb.sqrt() }) - cb
            } * (cs * 2. - 1.) + cb)
    }

    /// Subtracts the darker of the two constituent colors from the lighter color.
    /// Painting with white inverts the backdrop color; painting with black produces no change.
    #[inline] pub fn difference(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| (cb - cs).abs())
    }

    /// Produces an effect similar to that of the Difference mode but lower in contrast.
    /// Painting with white inverts the backdrop color; painting with black produces no change
    #[inline] pub fn enclusion(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs| cb + cs - 2. * cb * cs)
    }

    /// Creates a color with the hue of the source color and
    /// the saturation and luminosity of the backdrop color.
    #[inline] pub fn hue(self, drop: Self) -> Self {    // synonymous with chroma?
        self.set_sat(drop.to_sat()).set_lum(drop.to_lum())
    }

    /// Creates a color with the saturation of the source color and the hue and luminosity
    /// of the backdrop color. Painting with this mode in an area of the backdrop that is
    /// a pure gray (no saturation) produces no change.
    #[inline] pub fn saturation(self, drop: Self) -> Self {
        let lum = drop.to_lum(); drop.set_sat(self.to_sat()).set_lum(lum)
    }

    /// Creates a color with the hue and saturation of the source color and the luminosity
    /// of the backdrop color. This preserves the gray levels of the backdrop and is useful
    /// for coloring monochrome images or tinting color images.
    #[inline] pub fn coloring(self, drop: Self) -> Self { self.set_lum(drop.to_lum()) }

    /// Creates a color with the luminosity of the source color and the hue and saturation
    /// of the backdrop color. This produces an inverse effect to that of the Color mode.
    #[inline] pub fn luminosity(self, drop: Self) -> Self { drop.set_lum(self.to_lum()) }

    /// Luma is the weighted average of gamma-corrected R, G, and B, based on their contribution
    /// to perceived lightness, long used as the monochromatic dimension in color TV broadcast.
    #[inline] fn to_lum(self) -> f32 { 0.299 * self.r + 0.587 * self.g + 0.114 * self.b }

    /// https://en.wikipedia.org/wiki/HSL_and_HSV
    #[inline] fn to_sat(self) -> f32 {
        self.r.max(self.g).max(self.b) - self.r.min(self.g).min(self.b)
    }

    #[inline] fn set_sat(mut self, sat: f32) -> Self {
        let (mut cmin, mut cmax) = if self.r < self.g {
            (&mut self.r, &mut self.g) } else { (&mut self.g, &mut self.r) };

        let  mut cmid = &mut self.b;
             if *cmid < *cmin {  cmid = cmin; cmin = &mut self.b }
        else if *cmax < *cmid {  cmid = cmax; cmax = &mut self.b }

        if  *cmid <  *cmax {
            *cmid = (*cmid - *cmin) * sat / (*cmax - *cmin);    *cmax = sat
        } else {     *cmid = 0.;    *cmax = 0.; }    *cmid = 0.;    self
    }

    #[inline] fn set_lum(mut self, lum: f32) -> Self {
        let l = self.to_sat();  let d = lum - l;
        let n = self.r.min(self.g).min(self.b) + d;
        let x = self.r.max(self.g).max(self.b) + d;

        if n < 0. {
            let op = |c| l + (((c - l) * c) / (l - n));
            self.r = op(self.r); self.g = op(self.g); self.b = op(self.b);
        }
        if 1. < x {
            let op = |c| l + (((c - l) * (1. - l)) / (x - l));
            self.r = op(self.r); self.g = op(self.g); self.b = op(self.b);
        }   self
    }

    /// Simply divides pixel values of one layer with the other, but it's useful for
    /// brightening photos if the colour is on grey or less.
    /// It is also useful for removing a colour tint from a photo.
    #[inline] pub fn divide(self, drop: Self) -> Self {     // similar to color dodge
        self.blend(drop, |cb, cs|
            if cs == 0. { 1. } else { (cb / cs).min(1.) })
    }

    /// Simply subtracts pixel values of one layer with the other.
    /// In case of negative values, black is displayed.
    #[inline] pub fn subtract(self, drop: Self) -> Self {   // synonymous with minus?
        self.blend(drop, |cb, cs| (cb - cs).max(0.))
    }

    /// Sums the value in the two layers and subtracts 1.
    /// Blending with white leaves the image unchanged.
    #[inline] pub fn linear_burn(self, drop: Self) -> Self {    // same as inverse substract
        self.blend(drop, |cb, cs| (cb + cs - 1.).max(0.))
    }

    /// simply adds pixel values of one layer with the other.
    /// In case of values above 1 (in the case of RGB), white is displayed.
    #[inline] pub fn linear_dodge(self, drop: Self) -> Self {   // same as additive/addition?
        self.blend(drop, |cb, cs| (cb + cs).min(1.))
    }

    /// Combines Linear Dodge and Linear Burn (rescaled so that neutral colors become middle gray).
    #[inline] pub fn linear_light(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs|
            if 0.5 <= cb { (cs + (cb - 0.5) * 2.).min(1.) } else { (cs + cb * 2. - 1.).max(0.) })
    }

    /// Combines Color Dodge and Color Burn (rescaled so that neutral colors become middle gray).
    #[inline] pub fn vivid_light(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs|
            if 0.5 <= cs { if cs == 1. { 1. } else { (cb / (1. - cs) / 2.).min(1.) }
            } else         if cs == 0. { 0. } else { 1. - ((1. - cb) / cs / 2.).min(1.) })
    }

    #[inline] pub fn hard_mix(self, drop: Self) -> Self {   // use vivid-light?
        self.blend(drop, |cb, cs| if 1. - cs < cb { 1. } else { 0. })
    }

    #[inline] pub fn pin_light(self, drop: Self) -> Self {
        self.blend(drop, |cb, cs|
            if 0.5 <= cs { cb.max(2. * (cs - 0.5)) } else { cb.min(2. * cs) })
    }

    #[inline] pub fn overwrite(self, drop: Self) -> Self { self.blend(drop, |cb, _| cb) }
    //  https://docs.unity3d.com/Packages/com.unity.shadergraph@6.9/manual/Blend-Node.html
    //  https://docs.krita.org/en/reference_manual/blending_modes.html
}

