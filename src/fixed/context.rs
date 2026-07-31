//! Stateful facade for the fixed-point rendering pipeline.

use crate::{
    canvas::{FixedGeometryWorkspace, FixedRenderOptions, FixedStrokePathOptions, PixmapMut,
        RenderError, prepare_fixed_stroke_path, render_native_paint_fixed,
        render_native_paint_fixed_clipped, render_native_paint_fixed_masked,
        render_native_path_fixed, render_native_path_fixed_clipped,
        render_native_path_fixed_masked},
    color::{PremulSRGBA8, SRGBA}, context::{Clip, DrawState},
    fixed::{flatten::FixedFlattenOptions, raster::FixedRasterWorkspace,
        stroke::FixedStrokeOptions},
    geometry::{Affine, FixedScalar, Path, Rect}, raster::{CoverageMask, FillRule},
    sampler::FixedPaintSampler, stroke::StrokePathWorkspace,
};

/// Caller-owned scratch for [`FixedContext`].
pub struct FixedContextWorkspace<'a> {
    pub path: StrokePathWorkspace<'a, FixedScalar>,
    pub geometry: FixedGeometryWorkspace<'a>,
    pub raster: FixedRasterWorkspace<'a>,
}

#[derive(Clone, Copy)] struct FixedSolidPaint(PremulSRGBA8);

impl FixedPaintSampler for FixedSolidPaint {
    fn sample_fixed(&self, _x: u32, _y: u32) -> PremulSRGBA8 { self.0 }
    fn solid_color_fixed(&self) -> Option<PremulSRGBA8> { Some(self.0) }
}

/// Stateful Q24.8 drawing facade.
///
/// Methods accepting [`FixedPaintSampler`] are no-FPU except rectangle
/// clipping, whose compatibility coverage adapter currently uses f32. Use a
/// pre-rasterized fixed path mask when the complete clip path must avoid an FPU.
pub struct FixedContext<'a, 'target, 'workspace, 'clip> {
    target: &'a mut PixmapMut<'target>,
    workspace: &'a mut FixedContextWorkspace<'workspace>,
    state: DrawState<FixedScalar, FixedFlattenOptions, FixedStrokeOptions, FixedSolidPaint>,
    clip: Clip<'clip>,
}

impl<'a, 'target, 'workspace, 'clip> FixedContext<'a, 'target, 'workspace, 'clip> {
    pub fn new(target: &'a mut PixmapMut<'target>,
        workspace: &'a mut FixedContextWorkspace<'workspace>) -> Self {
        Self {
            target, workspace,
            state: DrawState {
                transform: Affine::identity(), fill_rule: FillRule::NonZero,
                flatten: FixedFlattenOptions::default(),
                stroke: FixedStrokeOptions::default(),
                paint: FixedSolidPaint(SRGBA::black().premul_encoded()),
            },
            clip: Clip::None,
        }
    }

    pub fn target(&self) -> &PixmapMut<'target> { self.target }
    pub fn target_mut(&mut self) -> &mut PixmapMut<'target> { self.target }
    pub fn transform(&self) -> Affine<FixedScalar> { self.state.transform }
    pub fn fill_rule(&self) -> FillRule { self.state.fill_rule }
    pub fn flatten(&self) -> FixedFlattenOptions { self.state.flatten }
    pub fn stroke_options(&self) -> FixedStrokeOptions { self.state.stroke }

    pub fn set_transform(&mut self, transform: Affine<FixedScalar>) -> &mut Self {
        self.state.transform = transform; self
    }

    pub fn set_fill_rule(&mut self, fill_rule: FillRule) -> &mut Self {
        self.state.fill_rule = fill_rule; self
    }

    pub fn set_flatten(&mut self, flatten: FixedFlattenOptions) -> &mut Self {
        self.state.flatten = flatten; self
    }

    pub fn set_stroke(&mut self, stroke: FixedStrokeOptions) -> &mut Self {
        self.state.stroke = stroke; self
    }

    pub fn set_color(&mut self, color: SRGBA<u8>) -> &mut Self {
        self.state.paint = FixedSolidPaint(color.premul_encoded()); self
    }

    pub fn clear_clip(&mut self) -> &mut Self { self.clip = Clip::None; self }

    /// Uses the f32 compatibility rectangle coverage adapter.
    pub fn set_clip_rect(&mut self, rect: Rect) -> &mut Self {
        self.clip = Clip::Rect(rect); self
    }

    pub fn set_clip_mask(&mut self, mask: CoverageMask<'clip>) -> &mut Self {
        self.clip = Clip::Mask(mask); self
    }

    pub fn fill(&mut self, path: &Path<FixedScalar>) -> Result<(), RenderError> {
        let paint = self.state.paint;
        self.fill_with(path, &paint)
    }

    pub fn fill_with<S: FixedPaintSampler>(&mut self, path: &Path<FixedScalar>,
        paint: &S) -> Result<(), RenderError> {
        let (options, clip) = (FixedRenderOptions {
            transform: self.state.transform, flatten: self.state.flatten,
            fill_rule: self.state.fill_rule,
        }, self.clip);
        let workspace = &mut *self.workspace;
        match clip {
            Clip::None => render_native_path_fixed(path, paint, options, self.target,
                &mut workspace.geometry, &mut workspace.raster),
            Clip::Rect(rect) => render_native_path_fixed_clipped(path, paint, rect, options,
                self.target, &mut workspace.geometry, &mut workspace.raster),
            Clip::Mask(mask) => render_native_path_fixed_masked(path, paint, mask, options,
                self.target, &mut workspace.geometry, &mut workspace.raster),
        }
    }

    pub fn stroke(&mut self, path: &Path<FixedScalar>) -> Result<(), RenderError> {
        let paint = self.state.paint;
        self.stroke_with(path, &paint)
    }

    pub fn stroke_with<S: FixedPaintSampler>(&mut self, path: &Path<FixedScalar>,
        paint: &S) -> Result<(), RenderError> {
        let (options, clip) = (FixedStrokePathOptions {
            transform: self.state.transform, flatten: self.state.flatten,
            stroke: self.state.stroke,
        }, self.clip);
        let workspace = &mut *self.workspace;
        let line_count = prepare_fixed_stroke_path(
            path, options, &mut workspace.path, &mut workspace.geometry)?;
        let lines = &workspace.geometry.lines[..line_count];
        match clip {
            Clip::None => render_native_paint_fixed(
                lines, paint, FillRule::NonZero, self.target, &mut workspace.raster),
            Clip::Rect(rect) => render_native_paint_fixed_clipped(
                lines, paint, rect, FillRule::NonZero, self.target, &mut workspace.raster),
            Clip::Mask(mask) => render_native_paint_fixed_masked(
                lines, paint, mask, FillRule::NonZero, self.target, &mut workspace.raster),
        }
    }
}

#[cfg(test)] mod tests {
    use super::*;
    use crate::{edge::Edge, fixed::raster::{FixedLine, FixedSegment, FixedTrapezoid},
        geometry::PathBuilder, stroke::{StrokeContour, StrokePathWorkspace}};

    #[test] fn fixed_context_matches_state_clip_and_workspace_shape() {
        let fixed = FixedScalar::from_num;
        let mut builder = PathBuilder::new();
        builder.move_to((fixed(0), fixed(0))).line_to((fixed(2), fixed(0)))
            .line_to((fixed(2), fixed(2))).line_to((fixed(0), fixed(2)));
        let path = builder.build();
        let (mut points, mut contours) = (
            [(FixedScalar::ZERO, FixedScalar::ZERO).into(); 8],
            [StrokeContour::default(); 2],
        );
        let (mut edges, mut lines) = (
            [Edge::<FixedScalar>::default(); 32], [FixedLine::default(); 32],
        );
        let (mut segments, mut trapezoids, mut row_area) = (
            [FixedSegment::default(); 32], [FixedTrapezoid::default(); 16], [0; 4],
        );
        let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 32]);
        let mut workspace = FixedContextWorkspace {
            path: StrokePathWorkspace {
                points: &mut points, contours: &mut contours,
            },
            geometry: FixedGeometryWorkspace { edges: &mut edges, lines: &mut lines },
            raster: FixedRasterWorkspace {
                segments: &mut segments, trapezoids: &mut trapezoids,
                row_area: &mut row_area, strip_offsets: &mut strip_offsets,
                strip_indices: &mut strip_indices,
            },
        };
        let mask_data = [
            255, 128, 0, 0,
            255, 128, 0, 0,
            0,   0,   0, 0,
        ];
        let mut pixels = [0; 4 * 3 * 4];
        let mut target = PixmapMut::new(&mut pixels, 4, 3, 16).unwrap();
        let mut context = FixedContext::new(&mut target, &mut workspace);
        context.set_color(SRGBA::new(255, 0, 0, 128))
            .set_transform(Affine::translate(fixed(1), FixedScalar::ZERO))
            .set_clip_mask(CoverageMask::new(&mask_data, 4, 3, 4).unwrap());
        context.fill(&path).unwrap();
        assert_eq!(
            &pixels[..16], &[0, 0, 0, 0, 64, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
}
