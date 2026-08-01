//! Floating-point color conversion accelerators.

use crate::color::{LinearPremulRGBA, LinearRGBA, PremulSRGBA8, SRGBA};

pub const SRGB8_ENCODE_LUT_SIZE: usize = 4096;

/// Caller-owned linear-to-sRGB8 lookup table for framebuffer presentation.
#[derive(Clone, Copy, Debug)] pub struct Srgb8Encoder<'a> { table: &'a [u8] }

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub struct Srgb8EncoderError {
    pub minimum: usize, pub actual: usize,
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

    pub fn encode(self, color: LinearPremulRGBA<f32>) -> PremulSRGBA8 {
        let [r, g, b, a] = color.to_array();
        if a <= 0.0 { return PremulSRGBA8::zeroed(); }
        let scale = (self.table.len() - 1) as f32;
        let channel = |value: f32| {
            let index = ((value / a).clamp(0.0, 1.0) * scale + 0.5) as usize;
            self.table[index]
        };
        let alpha = (a.clamp(0.0, 1.0) * u8::MAX as f32 + 0.5) as u8;
        SRGBA::new(channel(r), channel(g), channel(b), alpha).premul_encoded()
    }
}

#[cfg(test)] mod tests { use super::*;
    #[test] fn encoder_lut_tracks_the_exact_transfer_boundary() {
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
