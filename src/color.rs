
//! Color representations and transfer boundaries.
//!
//! # Architecture
//!
//! The type system tracks three independent properties which a raw four-channel
//! tuple cannot express:
//!
//! - **transfer**: [`SRGBA`] contains sRGB-encoded RGB, while [`LinearRGBA`]
//!   contains linear-light sRGB primaries; alpha is always linear opacity;
//! - **alpha association**: [`RGBA`] and the two straight wrappers keep RGB
//!   independent of alpha, while [`PremulRGBA`] and [`LinearPremulRGBA`] require
//!   each RGB channel to be no greater than alpha;
//! - **storage boundary**: [`EncodedPremulSRGBA8`] is the logical color returned
//!   by the current paint samplers; canvas owns the conversion to its raw
//!   premultiplied RGBA8888 framebuffer bytes.
//!
//! The color-correct reference path is:
//!
//! ```text
//! SRGBA<u8> (straight, encoded input)
//!     -> LinearRGBA<f32> (decode)
//!     -> LinearPremulRGBA<f32> (premultiply, interpolate)
//!     -> EncodedPremulSRGBA8 (encode and quantize at the framebuffer boundary)
//! ```
//!
//! Gradient interpolation follows this path. The current RGBA8888 canvas then
//! uses an explicitly marked compatibility bridge to composite those bytes in
//! the encoded domain. That last step is retained for the existing compact
//! integer framebuffer and is not colorimetrically equivalent to compositing
//! in linear light. A future linear framebuffer should keep
//! [`LinearPremulRGBA`] through compositing and encode only for presentation.
//! Generic [`PremulRGBA`] remains the low-level alpha-association primitive used
//! by the existing integer compositor; it does not itself identify a transfer
//! function.
//!
//! # Conventions and references
//!
//! [`RGBA::packed`] defines an endian-independent integer value such as
//! `0xAARRGGBB`; it does not define that integer's in-memory byte order or a
//! framebuffer pixel layout. Use explicit channel arrays for RGBA byte storage,
//! and `to_be_bytes`/`to_le_bytes` only when an external format specifies that
//! byte order.
//!
//! - [IEC 61966-2-1](https://webstore.iec.ch/en/publication/6169) defines sRGB
//!   and its encoding transformations.
//! - [CSS Color 4](https://www.w3.org/TR/css-color-4/#color-interpolation)
//!   distinguishes sRGB from linear-light sRGB and specifies interpolation
//!   after conversion to the chosen space and alpha premultiplication.
//! - [Compositing and Blending Level 1](https://www.w3.org/TR/compositing-1/#porterduffcompositingoperators)
//!   specifies Porter-Duff compositing, including source-over in premultiplied
//!   form and the separation between blending and compositing.
//! - Porter and Duff, [“Compositing Digital Images”](https://doi.org/10.1145/800031.808606),
//!   is the original four-channel compositing model.
//!
//! Names intentionally include transfer and alpha association at semantic
//! boundaries. The legacy [`RGBA`] and [`PremulRGBA`] names are transfer-neutral
//! building blocks; prefer [`SRGBA`], [`LinearRGBA`], [`LinearPremulRGBA`], or
//! [`EncodedPremulSRGBA8`] in new APIs whenever the interpretation is known.

/** ```
    use ugl_rs::color::RGBA;
    let cha = [0x11, 0x22, 0x33, 0xFF];
    let rgba = RGBA::<u8>::new(cha[0], cha[1], cha[2], cha[3]);
    assert!(rgba.r == cha[0] && rgba.g == cha[1] && rgba.b == cha[2] && rgba.a == cha[3]);
    assert_eq!(rgba, (cha[0], cha[1], cha[2]).into());

    assert_eq!(rgba.to_array(), cha);
    assert_eq!(rgba.to_array3(), cha[0..3]);
    assert_eq!(rgba.packed(), 0xFF112233);
    assert_eq!(rgba, 0xFF112233.into());
    assert_eq!(rgba, RGBA::try_from(&cha[0..3]).unwrap());
    assert_eq!(rgba, cha.into());
 ```
    https://github.com/linebender/color */
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(C)] pub struct RGBA<T: ColorChannel> { pub r: T, pub g: T, pub b: T, pub a: T, }

/// A straight-alpha color encoded with the sRGB transfer function.
#[derive(Clone, Copy, Debug, PartialEq)] #[allow(non_camel_case_types)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(transparent)] pub struct SRGBA<T: ColorChannel = u8>(RGBA<T>);

/// A straight-alpha linear-light sRGB color.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(transparent)] pub struct LinearRGBA<T: ColorChannel = f32>(RGBA<T>);

/// A premultiplied linear-light sRGB color used by the reference pipeline.
#[derive(Clone, Copy, Debug, PartialEq)] #[repr(transparent)]
pub struct LinearPremulRGBA<T: ColorChannel = f32>(PremulRGBA<T>);

impl<T: ColorChannel> Default for LinearPremulRGBA<T> {
    fn default() -> Self { Self(PremulRGBA::default()) }
}

/// An RGBA color whose RGB channels have already been multiplied by alpha.
///
/// This is a distinct type so straight-alpha [`RGBA`] values cannot accidentally
/// enter a premultiplied compositing pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(transparent)] pub struct PremulRGBA<T: ColorChannel>(RGBA<T>);

/// Premultiplied encoded sRGB bytes for the legacy compatibility path.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(transparent)] pub struct EncodedPremulSRGBA8(PremulRGBA<u8>);

pub const SRGB8_ENCODE_LUT_SIZE: usize = 4096;

/// Caller-owned linear-to-sRGB8 lookup table for framebuffer presentation.
#[derive(Clone, Copy, Debug)] pub struct Srgb8Encoder<'a> { table: &'a [u8] }

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub struct Srgb8EncoderError {
    pub minimum: usize, pub actual: usize,
}

pub trait ColorChannel: Copy { const MAX: Self; const MIN: Self; }
impl ColorChannel for u8  { const MAX: Self = u8 ::MAX; const MIN: Self = 0; }
impl ColorChannel for u16 { const MAX: Self = u16::MAX; const MIN: Self = 0; }
impl ColorChannel for f32 { const MAX: Self = 1.0;      const MIN: Self = 0.0; }

impl<T: ColorChannel> RGBA<T> {
    pub fn new(r: T, g: T, b: T, a: T) -> Self { Self { r, g, b, a } }

    pub fn to_array3(self) -> [T; 3] { [self.r, self.g, self.b] }
    pub fn to_array(self)  -> [T; 4] { [self.r, self.g, self.b, self.a] }

    pub fn zeroed() -> Self { Self { r: T::MIN, g: T::MIN, b: T::MIN, a: T::MIN } }
    pub fn white()  -> Self { Self { r: T::MAX, g: T::MAX, b: T::MAX, a: T::MAX } }
    pub fn black()  -> Self { Self { r: T::MIN, g: T::MIN, b: T::MIN, a: T::MAX } }
    pub fn green()  -> Self { Self { r: T::MIN, g: T::MAX, b: T::MIN, a: T::MAX } }
    pub fn blue()   -> Self { Self { r: T::MIN, g: T::MIN, b: T::MAX, a: T::MAX } }
    pub fn red()    -> Self { Self { r: T::MAX, g: T::MIN, b: T::MIN, a: T::MAX } }
    pub fn cyan()   -> Self { Self { r: T::MIN, g: T::MAX, b: T::MAX, a: T::MAX } }
    pub fn yellow() -> Self { Self { r: T::MAX, g: T::MAX, b: T::MIN, a: T::MAX } }
    pub fn purple() -> Self { Self { r: T::MAX, g: T::MIN, b: T::MAX, a: T::MAX } }
}

impl<T: ColorChannel> PremulRGBA<T> {
    pub fn alpha(&self) -> T { self.0.a }
    pub fn zeroed() -> Self { Self(RGBA::zeroed()) }
    pub fn to_array(self) -> [T; 4] { [self.0.r, self.0.g, self.0.b, self.0.a] }
}

impl<T: ColorChannel> Default for PremulRGBA<T> { fn default() -> Self { Self::zeroed() } }

impl<T: ColorChannel + PartialOrd> PremulRGBA<T> {
    /// Constructs a premultiplied color, rejecting RGB channels above alpha.
    pub fn new(r: T, g: T, b: T, a: T) -> Option<Self> {
        ([r, g, b, a].into_iter().all(|channel| channel >= T::MIN && channel <= T::MAX) &&
            r <= a && g <= a && b <= a).then(|| Self(RGBA::new(r, g, b, a)))
    }

    pub(crate) fn new_clamped(r: T, g: T, b: T, a: T) -> Self {
        let bound = |channel| {
            if channel >= T::MIN && channel <= T::MAX { channel }
            else if channel > T::MAX { T::MAX } else { T::MIN }
        };
        let a = bound(a);
        let clamp = |channel| { let channel = bound(channel);
            if channel <= a { channel } else { a }
        };
        Self(RGBA::new(clamp(r), clamp(g), clamp(b), a))
    }
}

/// Compatibility conversion for already-premultiplied channels.
///
/// Invalid RGB channels are clamped to alpha; new public code should prefer
/// [`PremulRGBA::new`] when invalid input must be reported.
impl<T: ColorChannel + PartialOrd> From<(T, T, T, T)> for PremulRGBA<T> {
    fn from((r, g, b, a): (T, T, T, T)) -> Self { Self::new_clamped(r, g, b, a) }
}

impl<T: ColorChannel> Default for RGBA<T> { fn default() -> Self { Self::black() } }

impl<T: ColorChannel> From<(T, T, T, T)> for RGBA<T> {
    fn from((r, g, b,  a): (T, T, T, T)) -> Self { Self { r, g, b, a } }
}

impl<T: ColorChannel> From<[T; 4]> for RGBA<T> {
    fn from([r, g, b, a]:  [T; 4]) -> Self { Self { r, g, b, a } }
}

impl<T: ColorChannel> From<(T, T, T)> for RGBA<T> {
    fn from((r, g, b): (T, T, T)) -> Self { Self { r, g, b, a: T::MAX } }
}

impl<T: ColorChannel> From<[T; 3]>    for RGBA<T> {
    fn from([r, g, b]: [T; 3])    -> Self { Self { r, g, b, a: T::MAX } }
}

impl<T: ColorChannel>  TryFrom<&[T]> for RGBA<T> {
    fn try_from(rgb: &[T]) -> Result<Self, Self::Error> { match rgb {
            [r, g, b] => Ok(Self::new(*r, *g, *b, T::MAX)),
            [r, g, b, a, ..] => Ok(Self::new(*r, *g, *b, *a)),
            _ => Err("an RGB(A) slice requires at least three channels"),
    } } type Error = &'static str;
}

impl From<RGBA<f32>> for RGBA<u8> {     // quantization
    fn from(clr: RGBA<f32>) -> Self {   const MAX: f32 = u8 ::MAX as _;
        let quantize = |channel: f32| (channel.clamp(0.0, 1.0) * MAX + 0.5) as _;
        Self { r: quantize(clr.r), g: quantize(clr.g),
               b: quantize(clr.b), a: quantize(clr.a) }
    }
}

impl From<RGBA<f32>> for RGBA<u16> {
    fn from(clr: RGBA<f32>) -> Self {   const MAX: f32 = u16::MAX as _;
        let quantize = |channel: f32| (channel.clamp(0.0, 1.0) * MAX + 0.5) as _;
        Self { r: quantize(clr.r), g: quantize(clr.g),
               b: quantize(clr.b), a: quantize(clr.a) }
    }
}

impl From<RGBA<u8>>  for RGBA<f32> {    // intensity/normalize
    fn from(clr: RGBA<u8>)  -> Self {   const MAX: f32 = u8 ::MAX as _;
        Self { r: clr.r as f32 / MAX, g: clr.g as f32 / MAX,
               b: clr.b as f32 / MAX, a: clr.a as f32 / MAX }
    }
}

impl From<RGBA<u16>> for RGBA<f32> {
    fn from(clr: RGBA<u16>) -> Self {   const MAX: f32 = u16::MAX as _;
        Self { r: clr.r as f32 / MAX, g: clr.g as f32 / MAX,
               b: clr.b as f32 / MAX, a: clr.a as f32 / MAX }
    }
}

impl From<RGBA<u16>> for RGBA<u8> {
    fn from(clr: RGBA<u16>) -> Self {
        let quantize = |channel| ((channel as u32 + 128) / 257) as _;
        Self { r: quantize(clr.r), g: quantize(clr.g),
               b: quantize(clr.b), a: quantize(clr.a) }
    }
}

impl From<RGBA<u8>>  for RGBA<u16> {
    fn from(clr: RGBA<u8>) -> Self {
        let expand = |channel| channel as u16 * 0x0101;
        Self { r: expand(clr.r), g: expand(clr.g), b: expand(clr.b), a: expand(clr.a) }
    }
}

impl From<u32> for RGBA<u8> {   // 0xAARRGGBB
    fn from(cpv: u32) -> Self {
        Self { r: (cpv >> 16) as _, g: (cpv >> 8) as _, b: cpv as _, a: (cpv >> 24) as _ }
    }
}

impl From<u64> for RGBA<u16> {  // 0xAAAARRRRGGGGBBBB
    fn from(cpv: u64) -> Self {
        Self { r: (cpv >> 32) as _, g: (cpv >> 16) as _, b: cpv as _,
               a: (cpv >> 48) as _ }
    }
}

impl RGBA<u8> {
    /// Returns the endian-independent numeric representation `0xAARRGGBB`.
    pub fn packed(&self) -> u32 {
        (self.a as u32) << 24 | (self.r as u32) << 16 |
        (self.g as u32) <<  8 |  self.b as u32
    }
    pub fn premul(self) -> PremulRGBA<u8> {
        let (half, alpha) = ((u8::MAX / 2) as u16, self.a as u16);
        let premul = |channel| ((channel as u16 * alpha + half) /  u8::MAX as u16) as _;
        (premul(self.r), premul(self.g), premul(self.b), self.a).into()
    }
}

impl PremulRGBA<u8> {
    /// Converts to straight alpha. This is lossy for translucent integer colors.
    pub fn unpremul(self) -> RGBA<u8> {
        let [r, g, b, a] = self.to_array();
        if a == 0 { return RGBA::zeroed(); }
        let expand = |channel| {
            ((channel as u32 * u8::MAX as u32 + a as u32 / 2) / a as u32)
                          .min(u8::MAX as _) as _
        };
        RGBA::new(expand(r), expand(g), expand(b), a)
    }
}

impl RGBA<u16> {
    /// Returns the endian-independent numeric representation `0xAAAARRRRGGGGBBBB`.
    pub fn packed(&self) -> u64 {
        (self.a as u64) << 48 | (self.r as u64) << 32 |
        (self.g as u64) << 16 |  self.b as u64
    }
    pub fn premul(self) -> PremulRGBA<u16> {
        let (half, alpha) = ((u16::MAX / 2) as u32, self.a as u32);
        let premul = |channel| ((channel as u32 * alpha + half) / u16::MAX as u32) as _;
        (premul(self.r), premul(self.g), premul(self.b), self.a).into()
    }
}

impl PremulRGBA<u16> {
    /// Converts to straight alpha. This is lossy for translucent integer colors.
    pub fn unpremul(self) -> RGBA<u16> {
        let [r, g, b, a] = self.to_array();
        if a == 0 { return RGBA::zeroed(); }
        let expand = |channel| {
            ((channel as u64 * u16::MAX as u64 + a as u64 / 2) / a as u64)
                          .min(u16::MAX as _) as _
        };
        RGBA::new(expand(r), expand(g), expand(b), a)
    }
}

impl RGBA<f32> {
    pub fn premul(self) -> PremulRGBA<f32> {
        (self.r * self.a, self.g * self.a, self.b * self.a, self.a).into()
    }
}

// IEC 61966-2-1 sRGB transfer pair for normalized RGB channels. The linear
// near-black segment avoids the power curve's steep slope at zero; above the
// paired 0.04045/0.0031308 breakpoints, decoding uses gamma 2.4 and encoding
// uses its reciprocal. Alpha is linear opacity and must not pass through these.
// https://en.wikipedia.org/wiki/Gamma_correction
fn srgb_decode(value: f32) -> f32 {
    if value <= 0.04045 { value / 12.92 }
    else { libm::powf((value + 0.055) / 1.055, 2.4) }
}

fn srgb_encode(value: f32) -> f32 {
    if value <= 0.003_130_8 { value * 12.92 }
    else { 1.055 * libm::powf(value, 1. / 2.4) - 0.055 }
}

impl SRGBA<u8> {
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self { Self(RGBA::new(r, g, b, a)) }
    pub fn to_array(self) -> [u8; 4] { self.0.to_array() }
    pub fn transparent() -> Self { Self(RGBA::zeroed()) }
    pub fn white()  -> Self { Self(RGBA::white()) }
    pub fn black()  -> Self { Self(RGBA::black()) }
    pub fn red()    -> Self { Self(RGBA::red()) }
    pub fn green()  -> Self { Self(RGBA::green()) }
    pub fn blue()   -> Self { Self(RGBA::blue()) }
    pub fn cyan()   -> Self { Self(RGBA::cyan()) }
    pub fn yellow() -> Self { Self(RGBA::yellow()) }
    pub fn purple() -> Self { Self(RGBA::purple()) }

    /// Premultiplies encoded sRGB bytes for the legacy high-throughput 8-bit path.
    ///
    /// This is not linear-light compositing; use [`Self::to_linear`] followed by
    /// [`LinearRGBA::premul`] for the reference color-correct path.
    pub fn premul_encoded(self) -> EncodedPremulSRGBA8 {
        EncodedPremulSRGBA8(self.0.premul())
    }

    /// Decodes sRGB RGB channels to linear light. Alpha remains a linear opacity.
    pub fn to_linear(self) -> LinearRGBA<f32> {
        const SCALE: f32 = 1.0 / u8::MAX as f32;
        LinearRGBA(RGBA::new(srgb_decode(self.0.r as f32 * SCALE),
                             srgb_decode(self.0.g as f32 * SCALE),
                             srgb_decode(self.0.b as f32 * SCALE),
            self.0.a as f32 * SCALE))
    }
}

impl Default for SRGBA<u8> { fn default() -> Self { Self::black() } }
impl From<(u8, u8, u8, u8)> for SRGBA<u8> {
    fn from((r, g, b, a): (u8, u8, u8, u8)) -> Self { Self::new(r, g, b, a) }
}
impl From<[u8; 4]> for SRGBA<u8> {
    fn from([r, g, b, a]: [u8; 4]) -> Self { Self::new(r, g, b, a) }
}
impl From<RGBA<u8>> for SRGBA<u8> { fn from(color: RGBA<u8>) -> Self { Self(color) } }
impl From<SRGBA<u8>> for RGBA<u8> { fn from(color: SRGBA<u8>) -> Self { color.0 } }

impl EncodedPremulSRGBA8 {
    /// Constructs encoded premultiplied bytes, rejecting RGB above alpha.
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Option<Self> {
        PremulRGBA::new(r, g, b, a).map(Self)
    }
    pub fn from_array([r, g, b, a]: [u8; 4]) -> Option<Self> { Self::new(r, g, b, a) }
    pub fn zeroed() -> Self { Self(PremulRGBA::zeroed()) }
    pub fn to_array(self) -> [u8; 4] { self.0.to_array() }
    /// Decodes encoded premultiplied bytes into linear-light premultiplied color.
    pub fn to_linear(self) -> LinearPremulRGBA<f32> {
        SRGBA::from(self.0.unpremul()).to_linear().premul()
    }
    pub(crate) fn into_legacy(self) -> PremulRGBA<u8> { self.0 }
}

impl Default for EncodedPremulSRGBA8 { fn default() -> Self { Self::zeroed() } }

impl LinearRGBA<f32> {
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self { Self(RGBA::new(r, g, b, a)) }
    pub fn premul(self) -> LinearPremulRGBA<f32> { LinearPremulRGBA(self.0.premul()) }
    pub fn to_array(self) -> [f32; 4] { self.0.to_array() }

    /// Encodes linear-light RGB as straight-alpha 8-bit sRGB.
    pub fn to_srgba8(self) -> SRGBA<u8> {
        let quantize = |value: f32| (value.clamp(0.0, 1.0) * u8::MAX as f32 + 0.5) as u8;
        SRGBA::new(quantize(srgb_encode(self.0.r)), quantize(srgb_encode(self.0.g)),
                   quantize(srgb_encode(self.0.b)), quantize(self.0.a))
    }
}

impl LinearPremulRGBA<f32> {
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Option<Self> {
        PremulRGBA::new(r, g, b, a).map(Self)
    }
    pub fn to_array(self) -> [f32; 4] { self.0.to_array() }
    pub fn alpha(&self) -> f32 { self.0.alpha() }
    pub fn unpremul(self) -> LinearRGBA<f32> { LinearRGBA(self.0.unpremul()) }
    pub fn to_encoded_srgba8(self) -> EncodedPremulSRGBA8 {
        self.unpremul().to_srgba8().premul_encoded()
    }

    pub(crate) fn scale(self, factor: f32) -> Self {
        let [r, g, b, a] = self.to_array();
        Self::from_valid(r * factor, g * factor, b * factor, a * factor)
    }

    pub(crate) fn src_over(self, dest: Self) -> Self {
        let ([sr, sg, sb, sa], [dr, dg, db, da]) = (self.to_array(), dest.to_array());
        let inverse = 1.0 - sa;
        Self::from_valid(
            sr + dr * inverse, sg + dg * inverse, sb + db * inverse,
            sa + da * inverse)
    }

    pub(crate) fn lerp(self, other: Self, t: f32) -> Self {
        let (from, to) = (self.to_array(), other.to_array());
        let channel = |index| from[index] + (to[index] - from[index]) * t;
        Self::from_valid(channel(0), channel(1), channel(2), channel(3))
    }

    fn from_valid(r: f32, g: f32, b: f32, a: f32) -> Self {
        debug_assert!([r, g, b, a].into_iter().all(|channel|
            channel.is_finite() && (0.0..=1.0).contains(&channel)));
        debug_assert!(r <= a && g <= a && b <= a);
        Self(PremulRGBA(RGBA::new(r, g, b, a)))
    }
}

impl<'a> Srgb8Encoder<'a> {
    pub fn new(table: &'a mut [u8]) -> Result<Self, Srgb8EncoderError> {
        if table.len() < SRGB8_ENCODE_LUT_SIZE {
            return Err(Srgb8EncoderError {
                minimum: SRGB8_ENCODE_LUT_SIZE, actual: table.len(),
            });
        }
        let table = &mut table[..SRGB8_ENCODE_LUT_SIZE];
        let scale = (table.len() - 1) as f32;
        for (index, encoded) in table.iter_mut().enumerate() {
            *encoded = LinearRGBA::new(index as f32 / scale, 0.0, 0.0, 1.0)
                .to_srgba8().to_array()[0];
        }
        Ok(Self { table })
    }

    pub fn encode(self, color: LinearPremulRGBA<f32>) -> EncodedPremulSRGBA8 {
        let [r, g, b, a] = color.to_array();
        if a <= 0.0 { return EncodedPremulSRGBA8::zeroed(); }
        let scale = (self.table.len() - 1) as f32;
        let channel = |value: f32| {
            let index = ((value / a).clamp(0.0, 1.0) * scale + 0.5) as usize;
            self.table[index]
        };
        let alpha = (a.clamp(0.0, 1.0) * u8::MAX as f32 + 0.5) as u8;
        SRGBA::new(channel(r), channel(g), channel(b), alpha).premul_encoded()
    }
}

impl From<SRGBA<u8>> for LinearRGBA<f32> {
    fn from(color: SRGBA<u8>) -> Self { color.to_linear() }
}

impl From<LinearRGBA<f32>> for SRGBA<u8> {
    fn from(color: LinearRGBA<f32>) -> Self { color.to_srgba8() }
}

impl PremulRGBA<f32> {
    /// Converts to straight alpha, normalizing transparent colors to transparent black.
    pub fn unpremul(self) -> RGBA<f32> {
        let [r, g, b, a] = self.to_array();
        if a <= 0.0 { return RGBA::zeroed(); }
        RGBA::new((r / a).clamp(0.0, 1.0), (g / a).clamp(0.0, 1.0),
                  (b / a).clamp(0.0, 1.0), a)
    }
}

impl From<RGBA<u8>>  for PremulRGBA<u8> {
    fn from(color: RGBA<u8>)  -> Self { color.premul() }
}

impl From<RGBA<u16>> for PremulRGBA<u16> {
    fn from(color: RGBA<u16>) -> Self { color.premul() }
}

impl From<RGBA<f32>> for PremulRGBA<f32> {
    fn from(color: RGBA<f32>) -> Self { color.premul() }
}

#[cfg(test)] mod tests { use super::*;
    #[test] fn slice_conversion_checks_length() {
        assert!(RGBA::<u8>::try_from(&[][..]).is_err());
        assert!(RGBA::<u8>::try_from(&[1, 2][..]).is_err());
        assert_eq!(RGBA::<u8>::try_from(&[1, 2, 3][..]), Ok((1, 2, 3, 255).into()));
        assert_eq!(RGBA::<u8>::try_from(&[1, 2, 3, 4, 5][..]), Ok((1, 2, 3, 4).into()));
    }

    #[test] fn integer_conversions_preserve_channel_extrema() {
        let white8  = RGBA::<u8> ::white();
        let white16 = RGBA::<u16>::white();
        assert_eq!(RGBA::<u16>::from(white8), white16);
        assert_eq!(RGBA::<u8>::from(white16), white8);
        assert_eq!(RGBA::<u8>::from(RGBA::new(128_u16, 255, 32_768, 65_535)),
                   RGBA::new(0, 1, 128, 255));
        assert_eq!(RGBA::<u8>::from(RGBA::new(-1.0, 0.5, 2.0, f32::NAN)),
                   RGBA::new(0, 128, 255, 0));
    }

    #[test] fn premultiplication_preserves_opaque_channels() {
        assert_eq!(RGBA::<u8> ::white().premul().to_array(), RGBA::<u8> ::white().to_array());
        assert_eq!(RGBA::<u16>::white().premul().to_array(), RGBA::<u16>::white().to_array());
        assert_eq!(RGBA::<u8>::new(255, 127, 1, 0).premul(), PremulRGBA::zeroed());
        assert_eq!(RGBA::<u16>::new(65535, 32767, 1, 0).premul(), PremulRGBA::zeroed());
    }

    #[test] fn unpremultiplication_is_explicit_lossy_and_normalizes_transparent_rgb() {
        let color8 = RGBA::<u8>::new(200, 100, 50, 128);
        let restored8 = color8.premul().unpremul();
        for (actual, expected) in restored8.to_array3().into_iter()
            .zip(color8.to_array3()) {
            assert!(actual.abs_diff(expected) <= 1);
        }
        assert_eq!(restored8.a, color8.a);

        let color16 = RGBA::<u16>::new(50_000, 30_000, 10_000, 32_768);
        let restored16 = color16.premul().unpremul();
        for (actual, expected) in restored16.to_array3().into_iter()
            .zip(color16.to_array3()) {
            assert!(actual.abs_diff(expected) <= 1);
        }
        assert_eq!(restored16.a, color16.a);

        let transparent: PremulRGBA<u8> = (200, 100, 50, 0).into();
        assert_eq!(transparent.unpremul(), RGBA::zeroed());
        assert_eq!(RGBA::<f32>::new(0.4, 0.2, 0.1, 0.5).premul().unpremul(),
                          RGBA::new(0.4, 0.2, 0.1, 0.5));
    }

    #[test] fn packed_values_are_numeric_and_endian_independent() {
        let rgba8 = RGBA::new(0x11_u8, 0x22, 0x33, 0xFF);
        assert_eq!(rgba8.packed(), 0xFF11_2233);
        assert_eq!(RGBA::from(rgba8.packed()), rgba8);
        let rgba16 = RGBA::new(0x1111_u16, 0x2222, 0x3333, 0xFFFF);
        assert_eq!(rgba16.packed(), 0xFFFF_1111_2222_3333);
        assert_eq!(RGBA::from(rgba16.packed()), rgba16);
    }

    #[test] fn premultiplied_construction_maintains_rgb_below_alpha() {
        assert_eq!(PremulRGBA::new(100_u8, 50, 25, 100).unwrap().to_array(),
                   [100, 50, 25, 100]);
        assert_eq!(PremulRGBA::new(101_u8, 50, 25, 100), None);
        assert_eq!(EncodedPremulSRGBA8::new(100, 50, 25, 100).unwrap().to_array(),
                   [100, 50, 25, 100]);
        assert_eq!(EncodedPremulSRGBA8::new(101, 50, 25, 100), None);
        let clamped: PremulRGBA<u8> = (200, 100, 50, 80).into();
        assert_eq!(clamped.to_array(), [80, 80, 50, 80]);
        assert_eq!(PremulRGBA::new(f32::NAN, 0.0, 0.0, 1.0), None);
    }

    #[test] fn explicit_srgb_boundaries_use_the_standard_transfer_function() {
        assert!((srgb_decode(0.5) - 0.214_041_14).abs() < 1e-6);
        assert!((srgb_decode(0.04045) - 0.003_130_805).abs() < 1e-7);
        assert!((srgb_encode(0.003_130_8) - 0.040_449_936).abs() < 1e-7);

        let encoded = SRGBA::new(128, 64, 32, 96);
        let linear = encoded.to_linear();
        assert!((linear.to_array()[3] - 96.0 / 255.0).abs() < f32::EPSILON);
        let restored = linear.to_srgba8();
        assert_eq!(restored, encoded);
        assert_eq!(encoded.premul_encoded().to_array(), [48, 24, 12, 96]);
        let linear_premul = linear.premul();
        assert_eq!(linear_premul.unpremul().to_srgba8(), encoded);

        let half_linear = LinearRGBA::new(0.5, 0.5, 0.5, 1.0).to_srgba8();
        assert_eq!(half_linear.to_array(), [188, 188, 188, 255]);
    }

    #[test] fn linear_premultiplied_arithmetic_preserves_its_invariant() {
        let mut state = 0x243F_6A88_u32;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / 0x00FF_FFFF_u32 as f32
        };
        let valid = |color: LinearPremulRGBA<f32>| {
            let [r, g, b, a] = color.to_array();
            [r, g, b, a].into_iter().all(|channel|
                channel.is_finite() && (0.0..=1.0).contains(&channel))
                && r <= a && g <= a && b <= a
        };

        for _ in 0..4096 {
            let (a, b) = (next(), next());
            let lhs = LinearPremulRGBA::new(next() * a, next() * a, next() * a, a).unwrap();
            let rhs = LinearPremulRGBA::new(next() * b, next() * b, next() * b, b).unwrap();
            let (factor, t) = (next(), next());
            assert!(valid(lhs.scale(factor)));
            assert!(valid(lhs.src_over(rhs)));
            assert!(valid(lhs.lerp(rhs, t)));
        }
    }

    #[test] fn srgb8_encoder_lut_tracks_the_exact_transfer_boundary() {
        assert_eq!(Srgb8Encoder::new(&mut [0; 16]).unwrap_err(),
            Srgb8EncoderError { minimum: SRGB8_ENCODE_LUT_SIZE, actual: 16 });
        let mut table = [0; SRGB8_ENCODE_LUT_SIZE];
        let encoder = Srgb8Encoder::new(&mut table).unwrap();
        for step in 0..=u16::MAX {
            let value = step as f32 / u16::MAX as f32;
            let color = LinearRGBA::new(value, value, value, 1.0).premul();
            let (actual, exact) =
                (encoder.encode(color).to_array(), color.to_encoded_srgba8().to_array());
            assert!(actual[0].abs_diff(exact[0]) <= 1,
                "value={value}, actual={actual:?}, exact={exact:?}");
        }
        for alpha in [0.0, 0.01, 0.25, 0.5, 1.0] {
            for value in [0.0, 0.001, 0.003_130_8, 0.01, 0.18, 0.5, 1.0] {
                let color = LinearRGBA::new(value, value * 0.5, value * 0.25, alpha)
                    .premul();
                let (actual, exact) =
                    (encoder.encode(color).to_array(), color.to_encoded_srgba8().to_array());
                for channel in 0..4 {
                    assert!(actual[channel].abs_diff(exact[channel]) <= 1,
                        "actual={actual:?}, exact={exact:?}");
                }
            }
        }
    }
}
