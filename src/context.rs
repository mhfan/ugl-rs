//! Stateful drawing facades over the allocation-free rendering pipelines.

use crate::{
    canvas::{DashedStrokePathOptions, DashedStrokePlanningWorkspace,
        DashedStrokeRequirements, DashedStrokeWorkspace, RenderOptions,
        RenderRequirements, RenderWorkspace, StrokePathOptions, StrokePlanningWorkspace,
        StrokeRequirements, StrokeWorkspace, PixmapMut, RenderError,
        dashed_stroke_requirements as plan_dashed_stroke, render_paint, render_requirements,
        render_paint_clipped, render_paint_masked,
        render_stroke_paint_dashed, render_stroke_paint_dashed_clipped,
        render_stroke_paint_dashed_masked,
        render_stroke_paint, render_stroke_paint_clipped,
        render_stroke_paint_masked},
    color::SRGBA, dash::{DashContour, DashPattern}, flatten::FlattenOptions,
    geometry::{Affine, Path, Point, Rect},
    raster::{CoverageMask, FillRule}, sampler::{PaintSampler, SolidPaint},
    stroke::StrokeOptions,
};

/// Caller-owned scratch for [`Context`].
///
/// Empty dash slices are valid when dashed strokes are not used.
pub struct ContextWorkspace<'a> {
    pub stroke: StrokeWorkspace<'a>,
    pub dash_points: &'a mut [Point],
    pub dash_contours: &'a mut [DashContour],
}

#[derive(Clone, Copy, Debug)] pub(crate) enum Clip<'a> {
    None, Rect(Rect), Mask(CoverageMask<'a>),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawState<T, F, S, P> {
    pub(crate) transform: Affine<T>, pub(crate) fill_rule: FillRule,
    pub(crate) flatten: F, pub(crate) stroke: S, pub(crate) paint: P,
}

/// Stateful analytic f32 drawing facade.
///
/// The context borrows both target and scratch storage. It allocates nothing,
/// and every draw call has the same capacity and error behavior as the
/// corresponding low-level function in [`crate::canvas`].
pub struct Context<'a, 'target, 'workspace, 'clip> {
    target: &'a mut PixmapMut<'target>,
    workspace: &'a mut ContextWorkspace<'workspace>,
    state: DrawState<f32, FlattenOptions, StrokeOptions, SolidPaint>,
    clip: Clip<'clip>,
}

impl<'a, 'target, 'workspace, 'clip> Context<'a, 'target, 'workspace, 'clip> {
    pub fn new(target: &'a mut PixmapMut<'target>,
        workspace: &'a mut ContextWorkspace<'workspace>) -> Self {
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

    pub fn fill_requirements(&self, path: &Path, edges: &mut [crate::edge::Edge]) ->
        Result<RenderRequirements, RenderError> {
        render_requirements(path, self.state.transform, RenderOptions {
            fill_rule: self.state.fill_rule, flatten: self.state.flatten,
        }, self.target.width(), self.target.height(), edges)
    }

    pub fn fill_with<S: PaintSampler>(&mut self, path: &Path, paint: &S) ->
        Result<(), RenderError> {
        let (transform, options, clip) = (
            self.state.transform,
            RenderOptions {
                fill_rule: self.state.fill_rule, flatten: self.state.flatten,
            },
            self.clip,
        );
        let target = &mut *self.target;
        let mut workspace = render_workspace(&mut self.workspace.stroke);
        match clip {
            Clip::None => render_paint(
                path, transform, paint, options, target, &mut workspace),
            Clip::Rect(rect) => render_paint_clipped(
                path, transform, paint, rect, options, target, &mut workspace),
            Clip::Mask(mask) => render_paint_masked(
                path, transform, paint, mask, options, target, &mut workspace),
        }
    }

    pub fn stroke(&mut self, path: &Path) -> Result<(), RenderError> {
        let paint = self.state.paint;
        self.stroke_with(path, &paint)
    }

    pub fn stroke_requirements(&self, path: &Path,
        workspace: &mut StrokePlanningWorkspace<'_>) ->
        Result<StrokeRequirements, RenderError> {
        crate::canvas::stroke_requirements(path, self.state.transform, StrokePathOptions {
            flatten: self.state.flatten, stroke: self.state.stroke,
        }, (self.target.width(), self.target.height()), workspace)
    }

    pub fn stroke_with<S: PaintSampler>(&mut self, path: &Path, paint: &S) ->
        Result<(), RenderError> {
        let (transform, options, clip) = (
            self.state.transform,
            StrokePathOptions {
                flatten: self.state.flatten, stroke: self.state.stroke,
            },
            self.clip,
        );
        let target = &mut *self.target;
        match clip {
            Clip::None => render_stroke_paint(
                path, transform, paint, options, target, &mut self.workspace.stroke),
            Clip::Rect(rect) => render_stroke_paint_clipped(
                path, transform, paint, rect, options, target, &mut self.workspace.stroke),
            Clip::Mask(mask) => render_stroke_paint_masked(
                path, transform, paint, mask, options, target, &mut self.workspace.stroke),
        }
    }

    pub fn stroke_dashed(&mut self, path: &Path, dash: DashPattern<'_>) ->
        Result<(), RenderError> {
        let paint = self.state.paint;
        self.stroke_dashed_with(path, &paint, dash)
    }

    pub fn dashed_stroke_requirements(&self, path: &Path, dash: DashPattern<'_>,
        workspace: &mut DashedStrokePlanningWorkspace<'_>) ->
        Result<DashedStrokeRequirements, RenderError> {
        plan_dashed_stroke(path, self.state.transform, DashedStrokePathOptions {
            flatten: self.state.flatten, stroke: self.state.stroke, dash,
        }, (self.target.width(), self.target.height()), workspace)
    }

    pub fn stroke_dashed_with<S: PaintSampler>(&mut self, path: &Path,
        paint: &S, dash: DashPattern<'_>) -> Result<(), RenderError> {
        let (transform, options, clip) = (
            self.state.transform,
            DashedStrokePathOptions {
                flatten: self.state.flatten, stroke: self.state.stroke, dash,
            },
            self.clip,
        );
        let target = &mut *self.target;
        let workspace = &mut *self.workspace;
        let mut dashed = DashedStrokeWorkspace {
            stroke: reborrow_stroke(&mut workspace.stroke),
            dash_points: workspace.dash_points,
            dash_contours: workspace.dash_contours,
        };
        match clip {
            Clip::None => render_stroke_paint_dashed(
                path, transform, paint, options, target, &mut dashed),
            Clip::Rect(rect) => render_stroke_paint_dashed_clipped(
                path, transform, paint, rect, options, target, &mut dashed),
            Clip::Mask(mask) => render_stroke_paint_dashed_masked(
                path, transform, paint, mask, options, target, &mut dashed),
        }
    }
}

fn render_workspace<'a>(
    workspace: &'a mut StrokeWorkspace<'_>) -> RenderWorkspace<'a> {
    RenderWorkspace {
        edges: workspace.edges, intersections: workspace.intersections,
        row_coverage: workspace.row_coverage, row_offsets: workspace.row_offsets,
        edge_indices: workspace.edge_indices,
    }
}

fn reborrow_stroke<'a>(workspace: &'a mut StrokeWorkspace<'_>) -> StrokeWorkspace<'a> {
    StrokeWorkspace {
        points: workspace.points, contours: workspace.contours, edges: workspace.edges,
        intersections: workspace.intersections, row_coverage: workspace.row_coverage,
        row_offsets: workspace.row_offsets, edge_indices: workspace.edge_indices,
    }
}

#[cfg(test)] mod tests {
    use super::*;
    use crate::{analytic::Intersection as AnalyticIntersection,
        dash::DashPattern, edge::Edge, geometry::{PathBuilder, Point},
        raster::CoverageMask,
        sampler::{GradientStop, GradientStops, LinearGradient, SpreadMode},
        stroke::StrokeContour,
    };

    struct Buffers {
        points: [Point; 8], contours: [StrokeContour; 2], edges: [Edge; 32],
        dash_points: [Point; 16], dash_contours: [DashContour; 8],
        intersections: [AnalyticIntersection; 32], row_coverage: [f32; 4],
        row_offsets: [u32; 5], edge_indices: [u32; 32],
    }

    impl Buffers {
        fn new() -> Self { Self {
            points: [Point::default(); 8], contours: [StrokeContour::default(); 2],
            dash_points: [Point::default(); 16],
            dash_contours: [DashContour::default(); 8],
            edges: [Edge::default(); 32],
            intersections: [AnalyticIntersection::default(); 32], row_coverage: [0.0; 4],
            row_offsets: [0; 5], edge_indices: [0; 32],
        } }

        fn workspace(&mut self) -> ContextWorkspace<'_> {
            ContextWorkspace {
                stroke: StrokeWorkspace {
                    points: &mut self.points, contours: &mut self.contours,
                    edges: &mut self.edges, intersections: &mut self.intersections,
                    row_coverage: &mut self.row_coverage,
                    row_offsets: &mut self.row_offsets,
                    edge_indices: &mut self.edge_indices,
                },
                dash_points: &mut self.dash_points,
                dash_contours: &mut self.dash_contours,
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
        let mut planning_edges = [Edge::default(); 8];
        let required = context.fill_requirements(&rectangle(), &mut planning_edges).unwrap();
        assert_eq!((required.edges, required.row_coverage), (2, 4));
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

    #[test] fn context_dashed_stroke_uses_current_paint_and_clip() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 1.0)).line_to((4.0, 1.0));
        let mut pixels = [0; 4 * 3 * 4];
        let mut target = PixmapMut::new(&mut pixels, 4, 3, 16).unwrap();
        let mut buffers = Buffers::new();
        let mut workspace = buffers.workspace();
        let mut context = Context::new(&mut target, &mut workspace);
        context.set_color(SRGBA::red())
            .set_stroke(StrokeOptions::new(1.0).unwrap())
            .set_clip_rect(Rect::from_ltrb(0.0, 0.0, 3.0, 3.0).unwrap());
        context.stroke_dashed(&builder.build(),
            DashPattern::new(&[1.0, 1.0], 0.0).unwrap()).unwrap();
        assert!(pixels[..4].iter().any(|&channel| channel != 0));
        assert_eq!(&pixels[4..8], &[0; 4]);
        assert!(pixels[8..12].iter().any(|&channel| channel != 0));
        assert_eq!(&pixels[12..16], &[0; 4]);
    }
}
