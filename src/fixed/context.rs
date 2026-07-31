//! Stateful facade for the fixed-point rendering pipeline.

use crate::{
    canvas::{Pixmap, RenderError},
    color::{PremulSRGBA8, SRGBA}, context::{Clip, DrawState, GlobalAlphaPaint},
    dash::DashContour, fixed::{Scalar, canvas::{DashedStrokePathOptions,
            DashedStrokeRequirements, DashedStrokeWorkspace, GeometryWorkspace,
            RenderOptions, RenderRequirements, StrokePathOptions,
            StrokePlanningWorkspace, StrokeRequirements,
            dashed_stroke_requirements as plan_dashed_stroke, render_requirements,
            prepare_dashed_stroke_path, prepare_stroke_path,
            render_paint, render_paint_clipped, render_paint_masked,
            render_path, render_path_clipped, render_path_masked},
        dash::Pattern as DashPattern,
        flatten::Options as FlattenOptions, raster::Workspace as RasterWorkspace,
        sampler::PaintSampler, stroke::Options as StrokeOptions},
    geometry::{Affine, Path, Point, Rect}, raster::{CoverageMask, FillRule},
    stroke::StrokePathWorkspace,
};

/// Caller-owned scratch borrowed by [`Context`].
pub struct Workspace<'a> {
    path: StrokePathWorkspace<'a, Scalar>,
    dash_points: &'a mut [Point<Scalar>],
    dash_contours: &'a mut [DashContour],
    geometry: GeometryWorkspace<'a>,
    raster: RasterWorkspace<'a>,
}

impl<'a> Workspace<'a> {
    pub fn new(path: StrokePathWorkspace<'a, Scalar>,
        dash_points: &'a mut [Point<Scalar>], dash_contours: &'a mut [DashContour],
        geometry: GeometryWorkspace<'a>, raster: RasterWorkspace<'a>) -> Self {
        Self { path, dash_points, dash_contours, geometry, raster }
    }
}

#[derive(Clone, Copy)] struct SolidPaint(PremulSRGBA8);

impl PaintSampler for SolidPaint {
    fn sample(&self, _x: u32, _y: u32) -> PremulSRGBA8 { self.0 }
    fn solid_color(&self) -> Option<PremulSRGBA8> { Some(self.0) }
}

/// Stateful Q24.8 drawing facade.
///
/// Methods accepting [`PaintSampler`] are no-FPU except rectangle
/// clipping, whose compatibility coverage adapter currently uses f32. Use a
/// pre-rasterized fixed path mask when the complete clip path must avoid an FPU.
pub struct Context<'a, 'target, 'workspace, 'clip> {
    target: &'a mut Pixmap<'target>,
    workspace: Workspace<'workspace>,
    state: DrawState<Scalar, FlattenOptions, StrokeOptions, SolidPaint>,
    clip: Clip<'clip>,
}

impl<'a, 'target, 'workspace, 'clip> Context<'a, 'target, 'workspace, 'clip> {
    pub fn new(target: &'a mut Pixmap<'target>,
        workspace: Workspace<'workspace>) -> Self {
        Self {
            target, workspace,
            state: DrawState {
                transform: Affine::identity(), fill_rule: FillRule::NonZero,
                flatten: FlattenOptions::default(),
                stroke: StrokeOptions::default(),
                paint: SolidPaint(SRGBA::black().premul_encoded()),
                global_alpha: u8::MAX,
            },
            clip: Clip::None,
        }
    }

    pub fn target(&self) -> &Pixmap<'target> { self.target }
    pub fn target_mut(&mut self) -> &mut Pixmap<'target> { self.target }
    pub fn transform(&self) -> Affine<Scalar> { self.state.transform }
    pub fn fill_rule(&self) -> FillRule { self.state.fill_rule }
    pub fn flatten(&self) -> FlattenOptions { self.state.flatten }
    pub fn stroke_options(&self) -> StrokeOptions { self.state.stroke }
    pub fn global_alpha(&self) -> u8 { self.state.global_alpha }

    pub fn set_transform(&mut self, transform: Affine<Scalar>) -> &mut Self {
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
        self.state.paint = SolidPaint(color.premul_encoded()); self
    }

    /// Sets the global opacity applied with integer arithmetic (`255` is opaque).
    pub fn set_global_alpha(&mut self, alpha: u8) -> &mut Self {
        self.state.global_alpha = alpha; self
    }

    pub fn clear_clip(&mut self) -> &mut Self { self.clip = Clip::None; self }

    /// Uses the f32 compatibility rectangle coverage adapter.
    pub fn set_clip_rect(&mut self, rect: Rect) -> &mut Self {
        self.clip = Clip::Rect(rect); self
    }

    pub fn set_clip_mask(&mut self, mask: CoverageMask<'clip>) -> &mut Self {
        self.clip = Clip::Mask(mask); self
    }

    pub fn fill(&mut self, path: &Path<Scalar>) -> Result<(), RenderError> {
        let paint = self.state.paint;
        self.fill_with(path, &paint)
    }

    pub fn fill_requirements(&self, path: &Path<Scalar>,
        workspace: &mut GeometryWorkspace<'_>) ->
        Result<RenderRequirements, RenderError> {
        render_requirements(path, RenderOptions {
            transform: self.state.transform, flatten: self.state.flatten,
            fill_rule: self.state.fill_rule,
        }, (self.target.width(), self.target.height()), workspace)
    }

    pub fn fill_with<S: PaintSampler>(&mut self, path: &Path<Scalar>,
        paint: &S) -> Result<(), RenderError> {
        let (options, clip) = (RenderOptions {
            transform: self.state.transform, flatten: self.state.flatten,
            fill_rule: self.state.fill_rule,
        }, self.clip);
        let (workspace, paint) = (&mut self.workspace,
            GlobalAlphaPaint::new(paint, self.state.global_alpha));
        match clip {
            Clip::None => render_path(path, &paint, options, self.target,
                &mut workspace.geometry, &mut workspace.raster),
            Clip::Rect(rect) => render_path_clipped(path, &paint, rect, options,
                self.target, &mut workspace.geometry, &mut workspace.raster),
            Clip::Mask(mask) => render_path_masked(path, &paint, mask, options,
                self.target, &mut workspace.geometry, &mut workspace.raster),
        }
    }

    pub fn stroke(&mut self, path: &Path<Scalar>) -> Result<(), RenderError> {
        let paint = self.state.paint;
        self.stroke_with(path, &paint)
    }

    pub fn stroke_requirements(&self, path: &Path<Scalar>,
        workspace: &mut StrokePlanningWorkspace<'_>) ->
        Result<StrokeRequirements, RenderError> {
        crate::fixed::canvas::stroke_requirements(path, StrokePathOptions {
            transform: self.state.transform, flatten: self.state.flatten,
            stroke: self.state.stroke,
        }, (self.target.width(), self.target.height()), workspace)
    }

    pub fn stroke_with<S: PaintSampler>(&mut self, path: &Path<Scalar>,
        paint: &S) -> Result<(), RenderError> {
        let (options, clip) = (StrokePathOptions {
            transform: self.state.transform, flatten: self.state.flatten,
            stroke: self.state.stroke,
        }, self.clip);
        let (workspace, paint) = (&mut self.workspace,
            GlobalAlphaPaint::new(paint, self.state.global_alpha));
        let usage = prepare_stroke_path(
            path, options, &mut workspace.path, &mut workspace.geometry)?;
        let lines = &workspace.geometry.lines[..usage.lines];
        match clip {
            Clip::None => render_paint(
                lines, &paint, FillRule::NonZero, self.target, &mut workspace.raster),
            Clip::Rect(rect) => render_paint_clipped(
                lines, &paint, rect, FillRule::NonZero, self.target, &mut workspace.raster),
            Clip::Mask(mask) => render_paint_masked(
                lines, &paint, mask, FillRule::NonZero, self.target, &mut workspace.raster),
        }
    }

    pub fn stroke_dashed(&mut self, path: &Path<Scalar>, dash: DashPattern<'_>) ->
        Result<(), RenderError> {
        let paint = self.state.paint;
        self.stroke_dashed_with(path, &paint, dash)
    }

    pub fn dashed_stroke_requirements(&self, path: &Path<Scalar>, dash: DashPattern<'_>,
        workspace: &mut DashedStrokeWorkspace<'_>) ->
        Result<DashedStrokeRequirements, RenderError> {
        plan_dashed_stroke(path, DashedStrokePathOptions {
            path: StrokePathOptions {
                transform: self.state.transform, flatten: self.state.flatten,
                stroke: self.state.stroke,
            },
            dash,
        }, (self.target.width(), self.target.height()), workspace)
    }

    pub fn stroke_dashed_with<S: PaintSampler>(&mut self, path: &Path<Scalar>,
        paint: &S, dash: DashPattern<'_>) -> Result<(), RenderError> {
        let (options, clip) = (DashedStrokePathOptions {
            path: StrokePathOptions {
                transform: self.state.transform, flatten: self.state.flatten,
                stroke: self.state.stroke,
            },
            dash,
        }, self.clip);
        let (workspace, paint) = (&mut self.workspace,
            GlobalAlphaPaint::new(paint, self.state.global_alpha));
        let mut dashed = DashedStrokeWorkspace {
            path: StrokePathWorkspace {
                points: workspace.path.points, contours: workspace.path.contours,
            },
            dash_points: workspace.dash_points,
            dash_contours: workspace.dash_contours,
            geometry: GeometryWorkspace {
                edges: workspace.geometry.edges, lines: workspace.geometry.lines,
            },
        };
        let usage = prepare_dashed_stroke_path(path, options, &mut dashed)?;
        let lines = &dashed.geometry.lines[..usage.lines];
        match clip {
            Clip::None => render_paint(
                lines, &paint, FillRule::NonZero, self.target, &mut workspace.raster),
            Clip::Rect(rect) => render_paint_clipped(
                lines, &paint, rect, FillRule::NonZero, self.target, &mut workspace.raster),
            Clip::Mask(mask) => render_paint_masked(
                lines, &paint, mask, FillRule::NonZero, self.target, &mut workspace.raster),
        }
    }
}

#[cfg(test)] mod tests {
    use super::*;
    use crate::{edge::Edge, fixed::raster::{Line, Segment, Trapezoid},
        geometry::PathBuilder, stroke::{StrokeContour, StrokePathWorkspace}};

    #[test] fn context_matches_state_clip_and_workspace_shape() {
        let fixed = Scalar::from_num;
        let mut builder = PathBuilder::new();
        builder.move_to((fixed(0), fixed(0))).line_to((fixed(2), fixed(0)))
            .line_to((fixed(2), fixed(2))).line_to((fixed(0), fixed(2)));
        let path = builder.build();
        let (mut points, mut contours) = (
            [(Scalar::ZERO, Scalar::ZERO).into(); 8],
            [StrokeContour::default(); 2],
        );
        let (mut dash_points, mut dash_contours) = (
            [(Scalar::ZERO, Scalar::ZERO).into(); 16],
            [DashContour::default(); 8],
        );
        let (mut edges, mut lines) = (
            [Edge::<Scalar>::default(); 32], [Line::default(); 32],
        );
        let (mut segments, mut trapezoids, mut row_area) = (
            [Segment::default(); 32], [Trapezoid::default(); 16], [0; 4],
        );
        let (mut strip_offsets, mut strip_indices) = ([0; 2], [0; 32]);
        let workspace = Workspace::new(StrokePathWorkspace {
                points: &mut points, contours: &mut contours,
            },
            &mut dash_points, &mut dash_contours,
            GeometryWorkspace { edges: &mut edges, lines: &mut lines },
            RasterWorkspace {
                segments: &mut segments, trapezoids: &mut trapezoids,
                row_area: &mut row_area, strip_offsets: &mut strip_offsets,
                strip_indices: &mut strip_indices,
            },
        );
        let mask_data = [
            255, 128, 0, 0,
            255, 128, 0, 0,
            0,   0,   0, 0,
        ];
        let mut pixels = [0; 4 * 3 * 4];
        let mut target = Pixmap::from_buffer(&mut pixels, 4, 3, 16).unwrap();
        let mut context = Context::new(&mut target, workspace);
        context.set_color(SRGBA::new(255, 0, 0, 128))
            .set_transform(Affine::translate(fixed(1), Scalar::ZERO))
            .set_clip_mask(CoverageMask::new(&mask_data, 4, 3, 4).unwrap());
        context.fill(&path).unwrap();
        assert_eq!(
            &pixels[..16], &[0, 0, 0, 0, 64, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
}
