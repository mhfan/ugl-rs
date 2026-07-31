//! Stateful drawing facades over the allocation-free rendering pipelines.

use alloc::vec::Vec;
use crate::{
    analytic::Intersection as AnalyticIntersection,
    canvas::{DashedStrokePathOptions, DashedStrokePlanningWorkspace,
        DashedStrokeRequirements, DashedStrokeWorkspace, RenderOptions,
        RenderRequirements, RenderWorkspace, StrokePathOptions, StrokePlanningWorkspace,
        StrokeRequirements, StrokeWorkspace, PixmapMut, RenderError,
        dashed_stroke_requirements as plan_dashed_stroke, rasterize_path_clip,
        render_paint, render_requirements,
        render_paint_clipped, render_paint_masked,
        render_stroke_paint_dashed, render_stroke_paint_dashed_clipped,
        render_stroke_paint_dashed_masked,
        render_stroke_paint, render_stroke_paint_clipped,
        render_stroke_paint_masked},
    color::SRGBA, dash::{DashContour, DashPattern}, edge::Edge, flatten::FlattenOptions,
    geometry::{Affine, Path, Point, Rect},
    raster::{CoverageMask, CoverageMaskMut, FillRule}, sampler::{PaintSampler, SolidPaint},
    stroke::{StrokeContour, StrokeOptions},
};

/// Caller-owned scratch borrowed by [`Context`].
///
/// Empty dash slices are valid when dashed strokes are not used.
pub struct Workspace<'a> {
    stroke: StrokeWorkspace<'a>,
    dash_points: &'a mut [Point],
    dash_contours: &'a mut [DashContour],
}

impl<'a> Workspace<'a> {
    /// Wraps explicitly managed low-level scratch for an allocation-free [`Context`].
    pub fn new(stroke: StrokeWorkspace<'a>, dash_points: &'a mut [Point],
        dash_contours: &'a mut [DashContour]) -> Self {
        Self { stroke, dash_points, dash_contours }
    }
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
    workspace: Workspace<'workspace>,
    state: DrawState<f32, FlattenOptions, StrokeOptions, SolidPaint>,
    clip: Clip<'clip>,
}

impl<'a, 'target, 'workspace, 'clip> Context<'a, 'target, 'workspace, 'clip> {
    pub fn new(target: &'a mut PixmapMut<'target>,
        workspace: Workspace<'workspace>) -> Self {
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
        let workspace = &mut self.workspace;
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

#[derive(Default)] struct CanvasStorage {
    points: Vec<Point>, contours: Vec<StrokeContour>, edges: Vec<Edge>,
    dash_points: Vec<Point>, dash_contours: Vec<DashContour>,
    intersections: Vec<AnalyticIntersection>, row_coverage: Vec<f32>,
    row_offsets: Vec<u32>, edge_indices: Vec<u32>,
}

impl CanvasStorage {
    fn workspace(&mut self) -> Workspace<'_> { Workspace::new(
        StrokeWorkspace {
            points: &mut self.points, contours: &mut self.contours,
            edges: &mut self.edges, intersections: &mut self.intersections,
            row_coverage: &mut self.row_coverage, row_offsets: &mut self.row_offsets,
            edge_indices: &mut self.edge_indices,
        }, &mut self.dash_points, &mut self.dash_contours)
    }

    fn grow_for(&mut self, error: RenderError) -> bool {
        let grow = |len: usize, required: usize| required.max(len.saturating_mul(2).max(8));
        match error {
            RenderError::EdgeCapacity { needed_at_least } =>
                self.edges.resize(grow(self.edges.len(), needed_at_least), Edge::default()),
            RenderError::StrokePointCapacity { needed_at_least } =>
                self.points.resize(grow(self.points.len(), needed_at_least), Point::default()),
            RenderError::StrokeContourCapacity { needed_at_least } => self.contours.resize(
                grow(self.contours.len(), needed_at_least), StrokeContour::default()),
            RenderError::DashPointCapacity { needed_at_least } => self.dash_points.resize(
                grow(self.dash_points.len(), needed_at_least), Point::default()),
            RenderError::DashContourCapacity { needed_at_least } => self.dash_contours.resize(
                grow(self.dash_contours.len(), needed_at_least), DashContour::default()),
            _ => return false,
        }
        true
    }

    fn prepare_render(&mut self, required: RenderRequirements) {
        self.edges.resize(required.edges, Edge::default());
        self.intersections.resize(required.intersections, AnalyticIntersection::default());
        self.row_coverage.resize(required.row_coverage, 0.0);
        self.row_offsets.resize(required.row_offsets, 0);
        self.edge_indices.resize(required.edge_indices, 0);
    }
}

/// Convenient stateful f32 renderer with automatically managed scratch storage.
///
/// `Canvas` plans and grows scratch before every draw, then delegates to the
/// allocation-free [`Context`]. Geometry or capacity failure therefore occurs
/// before the destination is modified. Use `Context` or [`crate::canvas`]
/// directly when scratch must be statically supplied.
pub struct Canvas<'target, 'clip> {
    target: PixmapMut<'target>, storage: CanvasStorage,
    state: DrawState<f32, FlattenOptions, StrokeOptions, SolidPaint>,
    clip: CanvasClip<'clip>,
}

enum CanvasClip<'a> {
    None, Rect(Rect), Borrowed(CoverageMask<'a>),
    Path { data: Vec<u8>, width: u32, height: u32 },
}

impl CanvasClip<'_> {
    fn as_clip(&self) -> Result<Clip<'_>, RenderError> { Ok(match self {
        Self::None => Clip::None,
        Self::Rect(rect) => Clip::Rect(*rect),
        Self::Borrowed(mask) => Clip::Mask(*mask),
        Self::Path { data, width, height } => Clip::Mask(
            CoverageMask::new(data, *width, *height, *width)
                .map_err(|_| RenderError::DimensionsOverflow)?),
    }) }
}

impl<'target, 'clip> Canvas<'target, 'clip> {
    pub fn new(data: &'target mut [u8], width: u32, height: u32, stride: u32) ->
        Result<Self, crate::canvas::PixmapError> {
        Ok(Self {
            target: PixmapMut::new(data, width, height, stride)?,
            storage: CanvasStorage::default(),
            state: DrawState {
                transform: Affine::identity(), fill_rule: FillRule::NonZero,
                flatten: FlattenOptions::default(), stroke: StrokeOptions::default(),
                paint: SolidPaint::new(SRGBA::black()),
            },
            clip: CanvasClip::None,
        })
    }

    pub fn target(&self) -> &PixmapMut<'target> { &self.target }
    pub fn target_mut(&mut self) -> &mut PixmapMut<'target> { &mut self.target }
    pub fn transform(&self) -> Affine { self.state.transform }
    pub fn fill_rule(&self) -> FillRule { self.state.fill_rule }
    pub fn flatten(&self) -> FlattenOptions { self.state.flatten }
    pub fn stroke_options(&self) -> StrokeOptions { self.state.stroke }

    pub fn set_transform(&mut self, value: Affine) -> &mut Self {
        self.state.transform = value; self
    }
    pub fn set_fill_rule(&mut self, value: FillRule) -> &mut Self {
        self.state.fill_rule = value; self
    }
    pub fn set_flatten(&mut self, value: FlattenOptions) -> &mut Self {
        self.state.flatten = value; self
    }
    pub fn set_stroke(&mut self, value: StrokeOptions) -> &mut Self {
        self.state.stroke = value; self
    }
    pub fn set_color(&mut self, value: SRGBA<u8>) -> &mut Self {
        self.state.paint = SolidPaint::new(value); self
    }
    pub fn clear_clip(&mut self) -> &mut Self { self.clip = CanvasClip::None; self }
    pub fn set_clip_rect(&mut self, value: Rect) -> &mut Self {
        self.clip = CanvasClip::Rect(value); self
    }
    pub fn set_clip_mask(&mut self, value: CoverageMask<'clip>) -> &mut Self {
        self.clip = CanvasClip::Borrowed(value); self
    }

    /// Rasterizes and retains an antialiased path clip using internal storage.
    pub fn set_clip_path(&mut self, path: &Path) -> Result<&mut Self, RenderError> {
        self.plan_fill(path)?;
        let (width, height) = (self.target.width(), self.target.height());
        let length = usize::try_from(width).ok().and_then(|width|
            usize::try_from(height).ok().and_then(|height| width.checked_mul(height)))
            .ok_or(RenderError::DimensionsOverflow)?;
        let mut data = alloc::vec![0; length];
        let mut mask = CoverageMaskMut::new(&mut data, width, height, width)
            .map_err(|_| RenderError::DimensionsOverflow)?;
        let mut context_workspace = self.storage.workspace();
        let mut workspace = render_workspace(&mut context_workspace.stroke);
        rasterize_path_clip(path, self.state.transform, RenderOptions {
            fill_rule: self.state.fill_rule, flatten: self.state.flatten,
        }, &mut mask, &mut workspace)?;
        self.clip = CanvasClip::Path { data, width, height };
        Ok(self)
    }

    fn plan_fill(&mut self, path: &Path) -> Result<(), RenderError> {
        let options = RenderOptions {
            fill_rule: self.state.fill_rule, flatten: self.state.flatten,
        };
        loop {
            match render_requirements(path, self.state.transform, options,
                self.target.width(), self.target.height(), &mut self.storage.edges) {
                Ok(required) => { self.storage.prepare_render(required); return Ok(()); }
                Err(error) if self.storage.grow_for(error) => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn plan_stroke(&mut self, path: &Path) -> Result<(), RenderError> {
        let options = StrokePathOptions {
            flatten: self.state.flatten, stroke: self.state.stroke,
        };
        loop {
            let result = crate::canvas::stroke_requirements(path, self.state.transform, options,
                (self.target.width(), self.target.height()), &mut StrokePlanningWorkspace {
                    points: &mut self.storage.points, contours: &mut self.storage.contours,
                    edges: &mut self.storage.edges,
                });
            match result {
                Ok(required) => { self.storage.prepare_render(required.render); return Ok(()); }
                Err(error) if self.storage.grow_for(error) => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn plan_dashed(&mut self, path: &Path, dash: DashPattern<'_>) ->
        Result<(), RenderError> {
        let options = DashedStrokePathOptions {
            flatten: self.state.flatten, stroke: self.state.stroke, dash,
        };
        loop {
            let result = plan_dashed_stroke(path, self.state.transform, options,
                (self.target.width(), self.target.height()),
                &mut DashedStrokePlanningWorkspace {
                    stroke: StrokePlanningWorkspace {
                        points: &mut self.storage.points, contours: &mut self.storage.contours,
                        edges: &mut self.storage.edges,
                    },
                    dash_points: &mut self.storage.dash_points,
                    dash_contours: &mut self.storage.dash_contours,
                });
            match result {
                Ok(required) => {
                    self.storage.prepare_render(required.stroke.render); return Ok(());
                }
                Err(error) if self.storage.grow_for(error) => {}
                Err(error) => return Err(error),
            }
        }
    }

    pub fn fill(&mut self, path: &Path) -> Result<(), RenderError> {
        let paint = self.state.paint; self.fill_with(path, &paint)
    }
    pub fn fill_with<S: PaintSampler>(&mut self, path: &Path, paint: &S) ->
        Result<(), RenderError> {
        self.plan_fill(path)?;
        let state = self.state;
        let (target, storage, clip) = (&mut self.target, &mut self.storage, &self.clip);
        let workspace = storage.workspace();
        let mut context = Context::new(target, workspace);
        context.state = state; context.clip = clip.as_clip()?; context.fill_with(path, paint)
    }
    pub fn stroke(&mut self, path: &Path) -> Result<(), RenderError> {
        let paint = self.state.paint; self.stroke_with(path, &paint)
    }
    pub fn stroke_with<S: PaintSampler>(&mut self, path: &Path, paint: &S) ->
        Result<(), RenderError> {
        self.plan_stroke(path)?;
        let state = self.state;
        let (target, storage, clip) = (&mut self.target, &mut self.storage, &self.clip);
        let workspace = storage.workspace();
        let mut context = Context::new(target, workspace);
        context.state = state; context.clip = clip.as_clip()?; context.stroke_with(path, paint)
    }
    pub fn stroke_dashed(&mut self, path: &Path, dash: DashPattern<'_>) ->
        Result<(), RenderError> {
        let paint = self.state.paint; self.stroke_dashed_with(path, &paint, dash)
    }
    pub fn stroke_dashed_with<S: PaintSampler>(&mut self, path: &Path,
        paint: &S, dash: DashPattern<'_>) -> Result<(), RenderError> {
        self.plan_dashed(path, dash)?;
        let state = self.state;
        let (target, storage, clip) = (&mut self.target, &mut self.storage, &self.clip);
        let workspace = storage.workspace();
        let mut context = Context::new(target, workspace);
        context.state = state; context.clip = clip.as_clip()?;
        context.stroke_dashed_with(path, paint, dash)
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

        fn workspace(&mut self) -> Workspace<'_> {
            Workspace::new(StrokeWorkspace {
                    points: &mut self.points, contours: &mut self.contours,
                    edges: &mut self.edges, intersections: &mut self.intersections,
                    row_coverage: &mut self.row_coverage,
                    row_offsets: &mut self.row_offsets,
                    edge_indices: &mut self.edge_indices,
                }, &mut self.dash_points, &mut self.dash_contours)
        }
    }

    fn rectangle() -> Path {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 0.0)).line_to((2.0, 0.0))
            .line_to((2.0, 2.0)).line_to((0.0, 2.0));
        builder.build()
    }

    #[test] fn canvas_manages_fill_stroke_and_dash_scratch_internally() {
        let mut pixels = [0; 4 * 4 * 4];
        let mut canvas = Canvas::new(&mut pixels, 4, 4, 16).unwrap();
        canvas.set_color(SRGBA::new(255, 0, 0, 128));
        canvas.fill(&rectangle()).unwrap();

        let mut line = PathBuilder::new();
        line.move_to((0.0, 2.0)).line_to((4.0, 2.0));
        let line = line.build();
        canvas.set_stroke(StrokeOptions::new(1.0).unwrap());
        canvas.stroke(&line).unwrap();
        canvas.stroke_dashed(&line,
            DashPattern::new(&[1.0, 1.0], 0.0).unwrap()).unwrap();
        assert!(canvas.target().pixel_bytes(0, 0).unwrap()[3] != 0);
    }

    #[test] fn canvas_owns_free_path_clip_storage() {
        let mut clip = PathBuilder::new();
        clip.move_to((0.0, 0.0)).line_to((2.0, 0.0))
            .line_to((2.0, 4.0)).line_to((0.0, 4.0));
        let mut shape = PathBuilder::new();
        shape.move_to((0.0, 0.0)).line_to((4.0, 0.0))
            .line_to((4.0, 4.0)).line_to((0.0, 4.0));
        let mut pixels = [0; 4 * 4 * 4];
        let mut canvas = Canvas::new(&mut pixels, 4, 4, 16).unwrap();
        canvas.set_color(SRGBA::red()).set_clip_path(&clip.build()).unwrap();
        canvas.fill(&shape.build()).unwrap();
        assert_ne!(canvas.target().pixel_bytes(0, 1).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(3, 1).unwrap(), [0; 4]);
    }

    #[test] fn context_fill_state_and_clip_match_low_level_pipeline() {
        let mut pixels = [0; 4 * 4 * 4];
        let mut target = PixmapMut::new(&mut pixels, 4, 4, 16).unwrap();
        let mut buffers = Buffers::new();
        let workspace = buffers.workspace();
        let mask_data = [
            255, 128, 0, 0,
            255, 128, 0, 0,
            0,   0,   0, 0,
            0,   0,   0, 0,
        ];
        let mut context = Context::new(&mut target, workspace);
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
        let workspace = buffers.workspace();
        let mut context = Context::new(&mut target, workspace);
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
        let workspace = buffers.workspace();
        let mut context = Context::new(&mut target, workspace);
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
