//! Stateful drawing facades over the allocation-free rendering pipelines.

use crate::{
    canvas::{AnalyticRenderOptions, AnalyticRenderWorkspace, AnalyticStrokeOptions,
        AnalyticStrokeWorkspace, PixmapMut, RenderError, render_paint_analytic,
        render_paint_analytic_clipped, render_paint_analytic_masked,
        render_stroke_paint_analytic, render_stroke_paint_analytic_clipped,
        render_stroke_paint_analytic_masked},
    color::SRGBA, flatten::FlattenOptions, geometry::{Affine, Path, Rect},
    raster::{CoverageMask, FillRule}, sampler::{PaintSampler, SolidPaint},
    stroke::StrokeOptions,
};

#[derive(Clone, Copy, Debug)] enum Clip<'a> {
    None, Rect(Rect), Mask(CoverageMask<'a>),
}

#[derive(Clone, Copy, Debug)]
struct DrawState<T, F, S, P> {
    transform: Affine<T>, fill_rule: FillRule, flatten: F, stroke: S,
    paint: P,
}

/// Stateful analytic f32 drawing facade.
///
/// The context borrows both target and scratch storage. It allocates nothing,
/// and every draw call has the same capacity and error behavior as the
/// corresponding low-level function in [`crate::canvas`].
pub struct Context<'a, 'target, 'workspace, 'clip> {
    target: &'a mut PixmapMut<'target>,
    workspace: &'a mut AnalyticStrokeWorkspace<'workspace>,
    state: DrawState<f32, FlattenOptions, StrokeOptions, SolidPaint>,
    clip: Clip<'clip>,
}

impl<'a, 'target, 'workspace, 'clip> Context<'a, 'target, 'workspace, 'clip> {
    pub fn new(target: &'a mut PixmapMut<'target>,
        workspace: &'a mut AnalyticStrokeWorkspace<'workspace>) -> Self {
        Self {
            target, workspace,
            state: DrawState {
                transform: Affine::identity(), fill_rule: FillRule::NonZero,
                flatten: FlattenOptions::default(), stroke: StrokeOptions::default(),
                paint: SolidPaint::new(SRGBA::black()),
            },
            clip: Clip::None,
        }
    }

    pub fn target(&self) -> &PixmapMut<'target> { self.target }
    pub fn target_mut(&mut self) -> &mut PixmapMut<'target> { self.target }
    pub fn transform(&self) -> Affine { self.state.transform }
    pub fn fill_rule(&self) -> FillRule { self.state.fill_rule }
    pub fn flatten(&self) -> FlattenOptions { self.state.flatten }
    pub fn stroke_options(&self) -> StrokeOptions { self.state.stroke }

    pub fn set_transform(&mut self, transform: Affine) -> &mut Self {
        self.state.transform = transform; self
    }

    pub fn set_fill_rule(&mut self, fill_rule: FillRule) -> &mut Self {
        self.state.fill_rule = fill_rule; self
    }

    pub fn set_flatten(&mut self, flatten: FlattenOptions) -> &mut Self {
        self.state.flatten = flatten; self
    }

    pub fn set_stroke(&mut self, stroke: StrokeOptions) -> &mut Self {
        self.state.stroke = stroke; self
    }

    pub fn set_color(&mut self, color: SRGBA<u8>) -> &mut Self {
        self.state.paint = SolidPaint::new(color); self
    }

    pub fn clear_clip(&mut self) -> &mut Self { self.clip = Clip::None; self }

    pub fn set_clip_rect(&mut self, rect: Rect) -> &mut Self {
        self.clip = Clip::Rect(rect); self
    }

    pub fn set_clip_mask(&mut self, mask: CoverageMask<'clip>) -> &mut Self {
        self.clip = Clip::Mask(mask); self
    }

    pub fn fill(&mut self, path: &Path) -> Result<(), RenderError> {
        let paint = self.state.paint;
        self.fill_with(path, &paint)
    }

    pub fn fill_with<S: PaintSampler>(&mut self, path: &Path, paint: &S) ->
        Result<(), RenderError> {
        let (transform, options, clip) = (
            self.state.transform,
            AnalyticRenderOptions {
                fill_rule: self.state.fill_rule, flatten: self.state.flatten,
            },
            self.clip,
        );
        let target = &mut *self.target;
        let mut workspace = analytic_workspace(&mut *self.workspace);
        match clip {
            Clip::None => render_paint_analytic(
                path, transform, paint, options, target, &mut workspace),
            Clip::Rect(rect) => render_paint_analytic_clipped(
                path, transform, paint, rect, options, target, &mut workspace),
            Clip::Mask(mask) => render_paint_analytic_masked(
                path, transform, paint, mask, options, target, &mut workspace),
        }
    }

    pub fn stroke(&mut self, path: &Path) -> Result<(), RenderError> {
        let paint = self.state.paint;
        self.stroke_with(path, &paint)
    }

    pub fn stroke_with<S: PaintSampler>(&mut self, path: &Path, paint: &S) ->
        Result<(), RenderError> {
        let (transform, options, clip) = (
            self.state.transform,
            AnalyticStrokeOptions {
                flatten: self.state.flatten, stroke: self.state.stroke,
            },
            self.clip,
        );
        let target = &mut *self.target;
        match clip {
            Clip::None => render_stroke_paint_analytic(
                path, transform, paint, options, target, self.workspace),
            Clip::Rect(rect) => render_stroke_paint_analytic_clipped(
                path, transform, paint, rect, options, target, self.workspace),
            Clip::Mask(mask) => render_stroke_paint_analytic_masked(
                path, transform, paint, mask, options, target, self.workspace),
        }
    }
}

fn analytic_workspace<'a>(
    workspace: &'a mut AnalyticStrokeWorkspace<'_>) -> AnalyticRenderWorkspace<'a> {
    AnalyticRenderWorkspace {
        edges: workspace.edges, intersections: workspace.intersections,
        row_coverage: workspace.row_coverage, row_offsets: workspace.row_offsets,
        edge_indices: workspace.edge_indices,
    }
}

#[cfg(feature = "fixed")] use crate::{
    canvas::{FixedGeometryWorkspace, FixedRenderOptions, FixedStrokePathOptions,
        prepare_fixed_stroke_path, render_native_paint_fixed,
        render_native_paint_fixed_clipped, render_native_paint_fixed_masked,
        render_native_path_fixed, render_native_path_fixed_clipped,
        render_native_path_fixed_masked},
    color::PremulSRGBA8, flatten_fixed::FixedFlattenOptions, geometry::FixedScalar,
    raster_fixed::FixedRasterWorkspace, sampler::FixedPaintSampler,
    stroke::StrokePathWorkspace, stroke_fixed::FixedStrokeOptions,
};

/// Caller-owned scratch for [`FixedContext`].
#[cfg(feature = "fixed")]
pub struct FixedContextWorkspace<'a> {
    pub path: StrokePathWorkspace<'a, FixedScalar>,
    pub geometry: FixedGeometryWorkspace<'a>,
    pub raster: FixedRasterWorkspace<'a>,
}

#[cfg(feature = "fixed")]
#[derive(Clone, Copy)] struct FixedSolidPaint(PremulSRGBA8);

#[cfg(feature = "fixed")] impl FixedPaintSampler for FixedSolidPaint {
    fn sample_fixed(&self, _x: u32, _y: u32) -> PremulSRGBA8 { self.0 }
    fn solid_color_fixed(&self) -> Option<PremulSRGBA8> { Some(self.0) }
}

/// Stateful Q24.8 drawing facade.
///
/// Methods accepting [`FixedPaintSampler`] are no-FPU except rectangle
/// clipping, whose compatibility coverage adapter currently uses f32. Use a
/// pre-rasterized fixed path mask when the complete clip path must avoid an FPU.
#[cfg(feature = "fixed")]
pub struct FixedContext<'a, 'target, 'workspace, 'clip> {
    target: &'a mut PixmapMut<'target>,
    workspace: &'a mut FixedContextWorkspace<'workspace>,
    state: DrawState<FixedScalar, FixedFlattenOptions, FixedStrokeOptions, FixedSolidPaint>,
    clip: Clip<'clip>,
}

#[cfg(feature = "fixed")]
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
    use crate::{analytic::AnalyticIntersection, edge::Edge, geometry::{PathBuilder, Point},
        raster::CoverageMask,
        sampler::{GradientStop, GradientStops, LinearGradient, SpreadMode},
        stroke::StrokeContour,
    };

    struct Buffers {
        points: [Point; 8], contours: [StrokeContour; 2], edges: [Edge; 32],
        intersections: [AnalyticIntersection; 32], row_coverage: [f32; 4],
        row_offsets: [u32; 5], edge_indices: [u32; 32],
    }

    impl Buffers {
        fn new() -> Self { Self {
            points: [Point::default(); 8], contours: [StrokeContour::default(); 2],
            edges: [Edge::default(); 32],
            intersections: [AnalyticIntersection::default(); 32], row_coverage: [0.0; 4],
            row_offsets: [0; 5], edge_indices: [0; 32],
        } }

        fn workspace(&mut self) -> AnalyticStrokeWorkspace<'_> {
            AnalyticStrokeWorkspace {
                points: &mut self.points, contours: &mut self.contours,
                edges: &mut self.edges, intersections: &mut self.intersections,
                row_coverage: &mut self.row_coverage, row_offsets: &mut self.row_offsets,
                edge_indices: &mut self.edge_indices,
            }
        }
    }

    fn rectangle() -> Path {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((2.0, 0.0))
            .line_to((2.0, 2.0)).line_to((0.0, 2.0));
        builder.build()
    }

    #[test] fn context_fill_state_and_clip_match_low_level_pipeline() {
        let mut pixels = [0; 4 * 4 * 4];
        let mut target = PixmapMut::new(&mut pixels, 4, 4, 16).unwrap();
        let mut buffers = Buffers::new();
        let mut workspace = buffers.workspace();
        let mask_data = [
            255, 128, 0, 0,
            255, 128, 0, 0,
            0,   0,   0, 0,
            0,   0,   0, 0,
        ];
        let mut context = Context::new(&mut target, &mut workspace);
        context.set_color(SRGBA::new(255, 0, 0, 128))
            .set_transform(Affine::translate(1.0, 0.0))
            .set_clip_mask(CoverageMask::new(&mask_data, 4, 4, 4).unwrap());
        context.fill(&rectangle()).unwrap();
        assert_eq!(
            &pixels[..16], &[0, 0, 0, 0, 64, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test] fn context_stroke_and_custom_paint_share_current_state() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 1.0)).line_to((4.0, 1.0));
        let path = builder.build();
        let stops = [GradientStop::new(0.0, SRGBA::red()),
                     GradientStop::new(1.0, SRGBA::blue())];
        let gradient = LinearGradient::new((0.0, 0.0), (4.0, 0.0),
            GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
        let mut pixels = [0; 4 * 3 * 4];
        let mut target = PixmapMut::new(&mut pixels, 4, 3, 16).unwrap();
        let mut buffers = Buffers::new();
        let mut workspace = buffers.workspace();
        let mut context = Context::new(&mut target, &mut workspace);
        context.set_stroke(StrokeOptions::new(2.0).unwrap())
            .set_clip_rect(Rect::from_ltrb(1.0, 0.0, 3.0, 3.0).unwrap());
        context.stroke_with(&path, &gradient).unwrap();
        for y in 0..2 {
            let row = &pixels[y * 16..(y + 1) * 16];
            assert_eq!(&row[..4], &[0; 4]);
            assert!(row[7] != 0 && row[11] != 0);
            assert_eq!(&row[12..], &[0; 4]);
        }
        assert_eq!(&pixels[32..], &[0; 16]);
    }

    #[cfg(feature = "fixed")]
    #[test] fn fixed_context_matches_state_clip_and_workspace_shape() {
        use crate::{
            raster_fixed::{FixedLine, FixedSegment, FixedTrapezoid},
            stroke::StrokePathWorkspace,
        };

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
