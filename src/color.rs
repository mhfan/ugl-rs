
pub type Color = RGBA<u8>;

/** ```
    use ugl_rs::color::RGBA;
    let cha = [0x11, 0x22, 0x33, 0xFF];
    let rgba = RGBA::<u8>::new(cha[0], cha[1], cha[2], cha[3]);
    assert!(rgba.r == cha[0] && rgba.g == cha[1] && rgba.b == cha[2] && rgba.a == cha[3]);
    assert_eq!(rgba, (cha[0], cha[1], cha[2]).into());

    assert_eq!(rgba.to_array(), cha);
    assert_eq!(rgba.to_arra3(), cha[0..3]);
    assert_eq!(rgba.packed(), 0xFF112233);
    assert_eq!(rgba, 0xFF112233.into());
    assert_eq!(rgba, cha[0..3] .into());
    assert_eq!(rgba, cha.into());
 ```
    https://github.com/linebender/color */
#[cfg(target_endian = "little")] #[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(C)] pub struct RGBA<T: ColorChannel> { pub b: T, pub g: T, pub r: T, pub a: T, }
//#[repr(C)] pub struct RGBA<T: ColorChannel> { pub r: T, pub g: T, pub b: T, pub a: T, }
#[cfg(target_endian =    "big")] #[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(C)] pub struct RGBA<T: ColorChannel> { pub a: T, pub r: T, pub g: T, pub b: T, }

pub trait ColorChannel: Copy { const MAX: Self; const MIN: Self; }
impl ColorChannel for u8  { const MAX: Self = u8 ::MAX; const MIN: Self = 0; }
impl ColorChannel for u16 { const MAX: Self = u16::MAX; const MIN: Self = 0; }
impl ColorChannel for f32 { const MAX: Self = 1.0;      const MIN: Self = 0.0; }

impl<T: ColorChannel> RGBA<T> {
    #[inline] pub fn new(r: T, g: T, b: T, a: T) -> Self { Self { r, g, b, a } }

    #[inline] pub fn to_arra3(self) -> [T; 3] { [self.r, self.g, self.b] }
    #[inline] pub fn to_array(self) -> [T; 4] { [self.r, self.g, self.b, self.a] }
        //unsafe { core::mem::transmute(self) }   // [b, g, r, a] or [a, r, g, b]

    #[inline] pub fn zeroed() -> Self { Self { r: T::MIN, g: T::MIN, b: T::MIN, a: T::MIN } }
    #[inline] pub fn white()  -> Self { Self { r: T::MAX, g: T::MAX, b: T::MAX, a: T::MAX } }
    #[inline] pub fn black()  -> Self { Self { r: T::MIN, g: T::MIN, b: T::MIN, a: T::MAX } }
    #[inline] pub fn green()  -> Self { Self { r: T::MIN, g: T::MAX, b: T::MIN, a: T::MAX } }
    #[inline] pub fn blue()   -> Self { Self { r: T::MIN, g: T::MIN, b: T::MAX, a: T::MAX } }
    #[inline] pub fn red()    -> Self { Self { r: T::MAX, g: T::MIN, b: T::MIN, a: T::MAX } }
    #[inline] pub fn cyan()   -> Self { Self { r: T::MIN, g: T::MAX, b: T::MAX, a: T::MAX } }
    #[inline] pub fn yellow() -> Self { Self { r: T::MAX, g: T::MAX, b: T::MIN, a: T::MAX } }
    #[inline] pub fn purple() -> Self { Self { r: T::MAX, g: T::MIN, b: T::MAX, a: T::MAX } }
}

impl<T: ColorChannel> Default for RGBA<T> { #[inline] fn default() -> Self { Self::black() } }

impl<T: ColorChannel> From<(T, T, T, T)> for RGBA<T> {
    #[inline] fn from((r, g, b,  a): (T, T, T, T)) -> Self { Self { r, g, b, a } }
        //unsafe { core::mem::transmute(rgba) }   // (b, g, r, a) or (a, r, g, b)
}

impl<T: ColorChannel> From<[T; 4]> for RGBA<T> {
    #[inline] fn from([r, g, b, a]: [T; 4]) -> Self { Self { r, g, b, a } }
        //unsafe { core::mem::transmute(rgba) }   // [b, g, r, a] or [a, r, g, b]
}

impl<T: ColorChannel> From<(T, T, T)> for RGBA<T> {
    #[inline] fn from((r, g, b): (T, T, T)) -> Self { Self { r, g, b, a: T::MAX } }
}

impl<T: ColorChannel> From<[T; 3]>    for RGBA<T> {
    #[inline] fn from([r, g, b]: [T; 3])    -> Self { Self { r, g, b, a: T::MAX } }
}

impl<T: ColorChannel> From<&[T]> for RGBA<T> {
    #[inline] fn from(rgb: &[T]) -> Self {   let len = rgb.len();
        Self { r: rgb[0], g: rgb[1], b: rgb[2], a:
            if 3 < len { rgb[3] } else if 2 < len { T::MAX } else { unreachable!() } }
    }
}

impl From<RGBA<f32>> for RGBA<u8> {     // quantization
    #[inline] fn from(clr: RGBA<f32>) -> Self {   const MAX: f32 = u8 ::MAX as _;
        Self { r: (clr.r * MAX + 0.5) as _, g: (clr.g * MAX + 0.5) as _,
               b: (clr.b * MAX + 0.5) as _, a: (clr.a * MAX + 0.5) as _ }
    }
}

impl From<RGBA<f32>> for RGBA<u16> {
    #[inline] fn from(clr: RGBA<f32>) -> Self {   const MAX: f32 = u16::MAX as _;
        Self { r: (clr.r * MAX + 0.5) as _, g: (clr.g * MAX + 0.5) as _,
               b: (clr.b * MAX + 0.5) as _, a: (clr.a * MAX + 0.5) as _ }
    }
}

impl From<RGBA<u8>>  for RGBA<f32> {    // intensity/normalize
    #[inline] fn from(clr: RGBA<u8>)  -> Self {   const MAX: f32 = u8 ::MAX as _;
        Self { r: clr.r as f32 / MAX, g: clr.g as f32 / MAX,
               b: clr.b as f32 / MAX, a: clr.a as f32 / MAX }
    }
}

impl From<RGBA<u16>> for RGBA<f32> {
    #[inline] fn from(clr: RGBA<u16>) -> Self {   const MAX: f32 = u16::MAX as _;
        Self { r: clr.r as f32 / MAX, g: clr.g as f32 / MAX,
               b: clr.b as f32 / MAX, a: clr.a as f32 / MAX }
    }
}

impl From<RGBA<u16>> for RGBA<u8> {
    #[inline] fn from(clr: RGBA<u16>) -> Self {
        Self { r: (clr.r >> 8) as _, g: (clr.g >> 8) as _,
               b: (clr.b >> 8) as _, a: (clr.a >> 8) as _ }
    }
}

impl From<RGBA<u8>>  for RGBA<u16> {
    #[inline] fn from(clr: RGBA<u8>) -> Self {
        Self { r: (clr.r as u16) << 8, g: (clr.g as u16) << 8,
               b: (clr.b as u16) << 8, a: (clr.a as u16) << 8 }
    }
}

impl From<u32> for RGBA<u8> {   // 0xAARRGGBB
        //Self { r: ((cpv >> 16) & 0xFF) as _, b: (cpv & 0xFF) as _,
        //       g: ((cpv >>  8) & 0xFF) as _, a: (cpv >> 24)  as _ }
    #[inline] fn from(cpv: u32) -> Self { unsafe { core::mem::transmute(cpv) } }
        //Self { r: (cpv >> 24)  as _, b: ((cpv >>  8) & 0xFF) as _,
        //       g: (cpv & 0xFF) as _, a: ((cpv >> 16) & 0xFF) as _ }
}

impl From<u64> for RGBA<u16> {  // 0xAAAARRRRGGGGBBBB
        //Self { r: ((cpv >> 32) & 0xFFFF) as _, b: (cpv & 0xFFFF) as _,
        //       g: ((cpv >> 16) & 0xFFFF) as _, a: (cpv >> 48)    as _ }
    #[inline] fn from(cpv: u64) -> Self { unsafe { core::mem::transmute(cpv) } }
        //Self { r: ((cpv >> 48)   as _, b: ((cpv >> 16) & 0xFFFF) as _,
        //       g: (cpv & 0xFFFF) as _, a: ((cpv >> 32) & 0xFFFF) as _ }
}

impl RGBA<u8> {
        //((self.a as u32) << 24) | ((self.r as u32) << 16) |
        //((self.g as u32) <<  8) |  (self.b as u32)
    #[inline] pub fn packed(&self) -> u32 { unsafe { core::mem::transmute(*self) } }
        //((self.b as u32) << 24) | ((self.g as u32) << 16) |
        //((self.r as u32) <<  8) |  (self.a as u32)
    pub fn mula(&self) -> Self {
        let (half, alpha) = ((u8::MAX / 2) as u16, self.a as u16);
        Self {  r: ((self.r as u16 * alpha + half) >> 8) as _,
                g: ((self.g as u16 * alpha + half) >> 8) as _,
                b: ((self.b as u16 * alpha + half) >> 8) as _, a: self.a }
    }
}

impl RGBA<u16> {
        //((self.a as u64) << 48) | ((self.r as u64) << 32) |
        //((self.g as u64) << 16) |  (self.b as u64)
    #[inline] pub fn packed(&self) -> u64 { unsafe { core::mem::transmute(*self) } }
        //((self.b as u64) << 48) | ((self.g as u64) << 32) |
        //((self.r as u64) << 16) |  (self.a as u64)
    pub fn mula(&self) -> Self {
        let (half, alpha) = ((u16::MAX / 2) as u32, self.a as u32);
        Self {  r: ((self.r as u32 * alpha + half) >> 16) as _,
                g: ((self.g as u32 * alpha + half) >> 16) as _,
                b: ((self.b as u32 * alpha + half) >> 16) as _, a: self.a }
    }
}

impl RGBA<f32> {    #![allow(unused)]
    pub fn mula(&self) -> Self {
        Self { r: self.r * self.a, g: self.g * self.a, b: self.b * self.a, a: self.a }
    }

    //  Gamma Correction: https://en.wikipedia.org/wiki/Gamma_correction
    #[inline] fn fast_gamma_expand(v: f32) -> f32 { v * v }
    #[inline] fn fast_gamma_encode(v: f32) -> f32 { v.sqrt() }

    #[inline] fn gamma_expand(v: f32) -> f32 { v.powf(2.2) }
    #[inline] fn gamma_encode(v: f32) -> f32 { v.powf(1. / 2.2) }
    // TODO: http://www.machinedlearnings.com/2011/06/fast-approximate-logarithm-exponential.html

    #[inline] fn srgb_gamma_expand(v: f32) -> f32 {
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    }   //  https://en.wikipedia.org/wiki/SRGB#Transformation
    #[inline] fn srgb_gamma_encode(v: f32) -> f32 {
        if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1. / 2.4) - 0.055 }
    }

    pub fn map2linear(&self) -> Self { Self {
        r: Self::fast_gamma_expand(self.r), g: Self::fast_gamma_expand(self.g),
        b: Self::fast_gamma_expand(self.b), a: self.a
    } }
    pub fn map2gamma (&self) -> Self { Self {
        r: Self::fast_gamma_encode(self.r), g: Self::fast_gamma_encode(self.g),
        b: Self::fast_gamma_encode(self.b), a: self.a
    } }
}

