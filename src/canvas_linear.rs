//! Linear-light premultiplied framebuffer and analytic compositing path.
//!
//! Unlike [`crate::canvas::PixmapMut`], this target retains `f32` linear-light
//! colors through source-over compositing. Encoding and RGBA8 quantization occur
//! only when [`LinearPixmapMut::encode_into`] presents into the compatibility
//! framebuffer.

use core::convert::Infallible;
use crate::{analytic::{AnalyticBinWorkspace, AnalyticWorkspace},
    canvas::{AnalyticRenderOptions, AnalyticRenderWorkspace, PixmapMut, RenderError,
        build_edges, rasterize_analytic},
    color::{LinearPremulRGBA, SRGBA}, geometry::{Affine, Path},
    raster::CoverageSink, sampler::{LinearPaintSampler, SolidPaint},
};

/// Borrowed premultiplied linear-light RGBA `f32` target.
///
/// `stride` is measured in pixels, not bytes. Caller-provided pixels must
/// already satisfy the [`LinearPremulRGBA`] invariant.
#[derive(Debug)] pub struct LinearPixmapMut<'a> {
    data: &'a mut [LinearPremulRGBA<f32>],
    width: u32, height: u32, stride: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)] pub enum LinearPixmapError {
    StrideTooSmall { minimum: u32, actual: u32 },
    BufferTooSmall { minimum: usize, actual: usize },
    DimensionsOverflow,
    DimensionsMismatch { source: (u32, u32), destination: (u32, u32) },
}

impl<'a> LinearPixmapMut<'a> {
    pub fn new(data: &'a mut [LinearPremulRGBA<f32>], width: u32, height: u32,
        stride: u32) -> Result<Self, LinearPixmapError> {
        if stride < width {
            return Err(LinearPixmapError::StrideTooSmall { minimum: width, actual: stride });
        }
        let (height_usize, stride_usize, width_usize) = (
            usize::try_from(height).map_err(|_| LinearPixmapError::DimensionsOverflow)?,
            usize::try_from(stride).map_err(|_| LinearPixmapError::DimensionsOverflow)?,
            usize::try_from(width).map_err(|_| LinearPixmapError::DimensionsOverflow)?,
        );
        let minimum = if height_usize == 0 { 0 } else {
            stride_usize.checked_mul(height_usize - 1)
                .and_then(|offset| offset.checked_add(width_usize))
                .ok_or(LinearPixmapError::DimensionsOverflow)?
        };
        if data.len() < minimum {
            return Err(LinearPixmapError::BufferTooSmall { minimum, actual: data.len() });
        }
        Ok(Self { data, width, height, stride })
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn stride(&self) -> u32 { self.stride }

    pub fn pixel(&self, x: u32, y: u32) -> Option<LinearPremulRGBA<f32>> {
        if x >= self.width || y >= self.height { return None; }
        Some(self.data[y as usize * self.stride as usize + x as usize])
    }

    /// Encodes the working buffer into premultiplied sRGB RGBA8888.
    pub fn encode_into(&self, destination: &mut PixmapMut<'_>) ->
        Result<(), LinearPixmapError> {
        if (self.width, self.height) != (destination.width(), destination.height()) {
            return Err(LinearPixmapError::DimensionsMismatch {
                source: (self.width, self.height),
                destination: (destination.width(), destination.height()),
            });
        }
        for y in 0..self.height {
            for x in 0..self.width {
                let color = self.data[y as usize * self.stride as usize + x as usize];
                destination.write_encoded_pixel(x, y, color.to_encoded_srgba8());
            }
        }
        Ok(())
    }

    fn blend_sampled_span<S: LinearPaintSampler>(&mut self, x: u32, y: u32, len: u32,
        sampler: &S, coverage: u8) {
        let factor = coverage as f32 / u8::MAX as f32;
        if let Some(color) = sampler.solid_color_linear() {
            let source = color.scale(factor);
            for pixel in &mut self.data[y as usize * self.stride as usize + x as usize..
                y as usize * self.stride as usize + (x + len) as usize] {
                *pixel = source.src_over(*pixel);
            }
            return;
        }
        let row = y as usize * self.stride as usize;
        for pixel_x in x..x + len {
            let source = sampler.sample_linear(pixel_x as f32 + 0.5, y as f32 + 0.5)
                .scale(factor);
            let pixel = &mut self.data[row + pixel_x as usize];
            *pixel = source.src_over(*pixel);
        }
    }
}

/// Renders a straight encoded-sRGB solid into a linear-light working target.
pub fn render_solid_analytic(path: &Path, transform: Affine, color: SRGBA<u8>,
    options: AnalyticRenderOptions, target: &mut LinearPixmapMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    render_paint_analytic(
        path, transform, &SolidPaint::from_srgba(color), options, target, workspace)
}

/// Renders a linear sampler through the exact-area analytic rasterizer.
pub fn render_paint_analytic<S: LinearPaintSampler>(path: &Path, transform: Affine,
    sampler: &S, options: AnalyticRenderOptions, target: &mut LinearPixmapMut<'_>,
    workspace: &mut AnalyticRenderWorkspace<'_>) -> Result<(), RenderError> {
    let edge_count = build_edges(path, transform, options.flatten, workspace.edges)?;
    let mut compositor = LinearPaintCompositor { target, sampler };
    rasterize_analytic(&workspace.edges[..edge_count], compositor.target.width,
        compositor.target.height, options.fill_rule, AnalyticWorkspace {
            intersections: workspace.intersections, row_coverage: workspace.row_coverage,
        }, AnalyticBinWorkspace {
            row_offsets: workspace.row_offsets, edge_indices: workspace.edge_indices,
        }, &mut compositor)
}

struct LinearPaintCompositor<'a, 'b, S> {
    target: &'a mut LinearPixmapMut<'b>, sampler: &'a S,
}

impl<S: LinearPaintSampler> CoverageSink for LinearPaintCompositor<'_, '_, S> {
    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        self.target.blend_sampled_span(x, y, len, self.sampler, coverage);
        Ok(())
    }
    type Error = Infallible;
}

#[cfg(test)] mod tests { use super::*;
    use crate::{analytic::AnalyticIntersection, edge::Edge, geometry::PathBuilder};

    fn rectangle() -> Path {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((1.0, 0.0))
            .line_to((1.0, 1.0)).line_to((0.0, 1.0)).close();
        builder.build()
    }

    fn render(color: SRGBA<u8>, target: &mut LinearPixmapMut<'_>) {
        let (mut edges, mut intersections, mut coverage) =
            ([Edge::default(); 4], [AnalyticIntersection::default(); 2], [0.0; 1]);
        render_solid_analytic(&rectangle(), Affine::identity(), color,
            AnalyticRenderOptions::default(), target, &mut AnalyticRenderWorkspace {
                intersections: &mut intersections, row_coverage: &mut coverage,
                edges: &mut edges, row_offsets: &mut [0; 2], edge_indices: &mut [0; 4],
            }).unwrap();
    }

    #[test] fn linear_pixmap_validates_pixel_stride_and_presentation_dimensions() {
        assert_eq!(LinearPixmapMut::new(&mut [LinearPremulRGBA::default(); 2], 2, 1, 1)
            .unwrap_err(), LinearPixmapError::StrideTooSmall { minimum: 2, actual: 1 });
        assert_eq!(LinearPixmapMut::new(&mut [LinearPremulRGBA::default(); 1], 2, 1, 2)
            .unwrap_err(), LinearPixmapError::BufferTooSmall { minimum: 2, actual: 1 });

        let mut pixels = [LinearPremulRGBA::default(); 2];
        let source = LinearPixmapMut::new(&mut pixels, 2, 1, 2).unwrap();
        let mut bytes = [0; 4];
        let mut destination = PixmapMut::new(&mut bytes, 1, 1, 4).unwrap();
        assert_eq!(source.encode_into(&mut destination).unwrap_err(),
            LinearPixmapError::DimensionsMismatch {
                source: (2, 1), destination: (1, 1),
            });
    }

    #[test] fn linear_source_over_differs_from_encoded_domain_and_encodes_once() {
        let mut pixels = [LinearPremulRGBA::default(); 1];
        let mut target = LinearPixmapMut::new(&mut pixels, 1, 1, 1).unwrap();
        render(SRGBA::blue(), &mut target);
        render(SRGBA::new(255, 0, 0, 128), &mut target);

        let [r, g, b, a] = target.pixel(0, 0).unwrap().to_array();
        assert!((r - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(g, 0.0);
        assert!((b - 127.0 / 255.0).abs() < 1e-6);
        assert_eq!(a, 1.0);

        let mut bytes = [0; 4];
        target.encode_into(&mut PixmapMut::new(&mut bytes, 1, 1, 4).unwrap()).unwrap();
        assert_eq!(bytes, [188, 0, 187, 255]);
        assert_ne!(bytes, [128, 0, 127, 255]);
    }
}
