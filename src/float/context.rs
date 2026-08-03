//! Stateful drawing facades over the allocation-free rendering pipelines.

use alloc::{rc::Rc, vec::Vec};
use crate::{
    common::{color::SRGBA, dash::DashContour,
        geometry::{Affine, Edge, Path, Point, Rect},
        raster::{CoverageMask, CoverageRun, CoverageStrip, CoverageStrips,
            FillRule, MaskKind, SparseCoverageSink, SparseStorage,
            clip_sparse_bounds, finish_sparse_coverage, intersect_sparse_masks,
            multiply_sparse_mask, sparse_mask_parts},
        render::{Clip, DrawState, GlobalAlphaPaint, validate_coverage_dimensions},
        stroke::StrokeContour,
        Pixmap, PixmapError, RenderError, SolidPaint},
    float::{analytic::{Cell, Intersection},
    canvas::{DashedStrokePathOptions, DashedStrokePlanningWorkspace,
        Compositing, DashedStrokeRequirements, DashedStrokeWorkspace, RenderOptions,
        RenderRequirements, RenderWorkspace, StrokePathOptions, StrokePlanningWorkspace,
        StrokeRequirements, StrokeWorkspace, stroke_requirements,
        build_edges, dashed_stroke_requirements as plan_dashed_stroke, edge_region,
        rasterize_built_region,
        render_paint_composited, render_requirements,
        render_stroke_paint_composited, render_stroke_paint_dashed_composited,
        },
        blend::CompositeMode, dash::DashPattern,
        flatten::FlattenOptions,
        math,
        sampler::PaintSampler, stroke::StrokeOptions},
};

/// Caller-owned scratch borrowed by [`CanvasRef`].
///
/// Empty dash slices are valid when dashed strokes are not used.
pub struct Workspace<'a> {
    stroke: StrokeWorkspace<'a>,
    dash_points: &'a mut [Point],
    dash_contours: &'a mut [DashContour],
}

impl<'a> Workspace<'a> {
    /// Wraps explicitly managed low-level scratch for an allocation-free [`CanvasRef`].
    pub fn new(stroke: StrokeWorkspace<'a>, dash_points: &'a mut [Point],
        dash_contours: &'a mut [DashContour]) -> Self {
        Self { stroke, dash_points, dash_contours }
    }
}

/// Stateful analytic f32 drawing facade.
///
/// `CanvasRef` borrows both target and scratch storage. It allocates nothing,
/// and every draw call has the same capacity and error behavior as the
/// corresponding low-level function in [`crate::float::canvas`].
pub struct CanvasRef<'a, 'target, 'workspace, 'clip> {
    target: &'a mut Pixmap<'target>,
    workspace: Workspace<'workspace>,
    state: DrawState<f32, FlattenOptions, StrokeOptions, SolidPaint>,
    comp_mode: CompositeMode,
    clip: Clip<'clip>,
}

impl<'a, 'target, 'workspace, 'clip> CanvasRef<'a, 'target, 'workspace, 'clip> {
    pub fn new(target: &'a mut Pixmap<'target>,
        workspace: Workspace<'workspace>) -> Self {
        Self {
            target, workspace,
            state: DrawState {
                transform: Affine::identity(), fill_rule: FillRule::NonZero,
                flatten: FlattenOptions::default(), stroke: StrokeOptions::default(),
                paint: SolidPaint::new(SRGBA::black()),
                global_alpha: u8::MAX,
            }, comp_mode: CompositeMode::SrcOver,
            clip: Clip::None,
        }
    }

    pub fn target(&self) -> &Pixmap<'target> { self.target }
    pub fn target_mut(&mut self) -> &mut Pixmap<'target> { self.target }
    pub fn transform(&self) -> Affine { self.state.transform }
    pub fn fill_rule(&self) -> FillRule { self.state.fill_rule }
    pub fn flatten(&self) -> FlattenOptions { self.state.flatten }
    pub fn stroke_options(&self) -> StrokeOptions { self.state.stroke }
    pub fn global_alpha(&self) -> u8 { self.state.global_alpha }

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

    /// Sets the global opacity applied to every paint sample (`255` is opaque).
    pub fn set_global_alpha(&mut self, alpha: u8) -> &mut Self {
        self.state.global_alpha = alpha; self
    }

    pub fn composite_mode(&self) -> CompositeMode { self.comp_mode }

    /// Selects the Porter-Duff or W3C blend operation used by subsequent draws.
    ///
    /// `Pixmap` stores encoded-premultiplied sRGBA8, so color blend functions
    /// are evaluated in encoded sRGB. Straight RGB exists only transiently
    /// inside the compositor; source and destination storage stay premultiplied.
    pub fn set_composite_mode(&mut self, mode: CompositeMode) -> &mut Self {
        self.comp_mode = mode; self
    }

    pub fn clear_clip(&mut self) -> &mut Self { self.clip = Clip::None; self }

    pub fn set_clip_rect(&mut self, rect: Rect) -> &mut Self {
        self.clip = Clip::Rect(rect); self
    }

    pub fn set_clip_mask(&mut self, mask: CoverageMask<'clip>) ->
        Result<&mut Self, RenderError> {
        validate_coverage_dimensions(mask.width(), mask.height(), self.target)?;
        self.clip = Clip::Mask(mask); Ok(self)
    }

    pub fn fill(&mut self, path: &Path) -> Result<(), RenderError> {
        let paint = self.state.paint;
        self.fill_with(path, &paint)
    }

    pub fn fill_requirements(&self, path: &Path, edges: &mut [Edge]) ->
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
        let (target, paint) = (&mut *self.target,
            GlobalAlphaPaint::new(paint, self.state.global_alpha));
        let mut workspace = render_workspace(&mut self.workspace.stroke);
        render_paint_composited(path, transform, &paint,
            Compositing { mode: self.comp_mode, clip }, options, target, &mut workspace)
    }

    pub fn stroke(&mut self, path: &Path) -> Result<(), RenderError> {
        let paint = self.state.paint;
        self.stroke_with(path, &paint)
    }

    pub fn stroke_requirements(&self, path: &Path,
        workspace: &mut StrokePlanningWorkspace<'_>) ->
        Result<StrokeRequirements, RenderError> {
        stroke_requirements(path, self.state.transform, StrokePathOptions {
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
        let (target, paint) = (&mut *self.target,
            GlobalAlphaPaint::new(paint, self.state.global_alpha));
        render_stroke_paint_composited(path, transform, &paint,
            Compositing { mode: self.comp_mode, clip }, options,
            target, &mut self.workspace.stroke)
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
        let (target, paint) = (&mut *self.target,
            GlobalAlphaPaint::new(paint, self.state.global_alpha));
        let workspace = &mut self.workspace;
        let mut dashed = DashedStrokeWorkspace {
            stroke: reborrow_stroke(&mut workspace.stroke),
            dash_points: workspace.dash_points,
            dash_contours: workspace.dash_contours,
        };
        render_stroke_paint_dashed_composited(path, transform, &paint,
            Compositing { mode: self.comp_mode, clip }, options, target, &mut dashed)
    }
}

fn render_workspace<'a>(
    workspace: &'a mut StrokeWorkspace<'_>) -> RenderWorkspace<'a> {
    RenderWorkspace {
        edges: workspace.edges, intersections: workspace.intersections,
        cells: workspace.cells, row_offsets: workspace.row_offsets,
        edge_indices: workspace.edge_indices,
    }
}

fn reborrow_stroke<'a>(workspace: &'a mut StrokeWorkspace<'_>) -> StrokeWorkspace<'a> {
    StrokeWorkspace {
        points: workspace.points, contours: workspace.contours, edges: workspace.edges,
        intersections: workspace.intersections, cells: workspace.cells,
        row_offsets: workspace.row_offsets, edge_indices: workspace.edge_indices,
    }
}

#[derive(Default)] struct CanvasStorage {
    points: Vec<Point>, contours: Vec<StrokeContour>, edges: Vec<Edge>,
    dash_points: Vec<Point>, dash_contours: Vec<DashContour>,
    intersections: Vec<Intersection>, cells: Vec<Cell>,
    row_offsets: Vec<u32>, edge_indices: Vec<u32>,
    clip_strips: Vec<CoverageStrip>, clip_runs: Vec<CoverageRun>,
}

impl CanvasStorage {
    fn workspace(&mut self) -> Workspace<'_> { Workspace::new(
        StrokeWorkspace {
            points: &mut self.points, contours: &mut self.contours,
            edges: &mut self.edges, intersections: &mut self.intersections,
            cells: &mut self.cells, row_offsets: &mut self.row_offsets,
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
        self.intersections.resize(required.intersections, Intersection::default());
        self.cells.resize(required.cells, Cell::default());
        self.row_offsets.resize(required.row_offsets, 0);
        self.edge_indices.resize(required.edge_indices, 0);
    }
}

/// Convenient stateful f32 renderer with automatically managed scratch storage.
///
/// `Canvas` plans and grows scratch before every draw, then delegates to the
/// allocation-free [`CanvasRef`]. Geometry or capacity failure therefore occurs
/// before the destination is modified. Use `CanvasRef` or [`crate::float::canvas`]
/// directly when scratch must be statically supplied.
pub struct Canvas<'target> {
    target: Pixmap<'target>, storage: CanvasStorage,
    state: DrawState<f32, FlattenOptions, StrokeOptions, SolidPaint>,
    comp_mode: CompositeMode, clip: CanvasClip, saved: Vec<SavedCanvasState>,
}

#[derive(Clone)] enum CanvasClip {
    None, Empty, Rect(Rect),
    Path { data: Rc<[u8]>, width: u32, height: u32,
        left: u32, top: u32, right: u32, bottom: u32, stride: u32 },
    Sparse { strips: Rc<Vec<CoverageStrip>>, runs: Rc<Vec<CoverageRun>>,
        width: u32, height: u32 },
}

#[derive(Clone)] struct SavedCanvasState {
    draw: DrawState<f32, FlattenOptions, StrokeOptions, SolidPaint>,
    comp_mode: CompositeMode, clip: CanvasClip,
}

impl CanvasClip {
    fn as_clip(&self) -> Result<Clip<'_>, RenderError> { Ok(match self {
        Self::None => Clip::None,
        Self::Empty => Clip::Rect(Rect::from_ltrb(0.0, 0.0, 0.0, 0.0)
            .expect("ordered empty rectangle")),
        Self::Rect(rect) => Clip::Rect(*rect),
        Self::Path { data, width, height, left, top, right, bottom, stride } => Clip::Mask(
            CoverageMask::from_region(data, (*width, *height),
                (*left, *top, *right, *bottom), *stride)
                .map_err(|_| RenderError::DimensionsOverflow)?),
        Self::Sparse { strips, runs, width, height } => Clip::SparseMask(
            CoverageStrips::from_parts(*width, *height, strips, runs)),
    }) }
}

fn intersect_rects(a: Rect, b: Rect) -> Option<Rect> {
    let (left, top) = (a.left().max(b.left()), a.top().max(b.top()));
    let (right, bottom) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    (left < right && top < bottom).then(||
        Rect::from_ltrb(left, top, right, bottom).expect("ordered rectangle intersection"))
}

fn mask_rect((left, top, right, bottom): (u32, u32, u32, u32)) -> Rect {
    Rect::from_ltrb(left as _, top as _, right as _, bottom as _)
        .expect("ordered mask bounds")
}

fn finish_sparse_clip(strips: Vec<CoverageStrip>, runs: Vec<CoverageRun>,
    width: u32, height: u32, storage: &mut CanvasStorage) ->
    Result<CanvasClip, RenderError> {
    Ok(match finish_sparse_coverage(strips, runs, width, height,
        &mut storage.clip_strips, &mut storage.clip_runs)
        .ok_or(RenderError::DimensionsOverflow)? {
        SparseStorage::Empty => CanvasClip::Empty,
        SparseStorage::OpaqueRect(bounds) => CanvasClip::Rect(mask_rect(bounds)),
        SparseStorage::Sparse { strips, runs } => CanvasClip::Sparse {
            strips: Rc::new(strips), runs: Rc::new(runs), width, height,
        },
        SparseStorage::Dense { data, left, top, right, bottom, stride } =>
            CanvasClip::Path {
                data: data.into(), width, height, left, top, right, bottom, stride,
            },
    })
}

fn rect_pixel_coverage(rect: Rect, x: u32, y: u32) -> u8 {
    let overlap = |from: f32, to: f32, pixel: u32| {
        (to.min(pixel as f32 + 1.0) - from.max(pixel as f32)).clamp(0.0, 1.0)
    };
    (overlap(rect.left(), rect.right(), x) * overlap(rect.top(), rect.bottom(), y) *
        255.0 + 0.5) as _
}

fn clip_sparse_rect(strips: &[CoverageStrip], runs: &[CoverageRun],
    width: u32, height: u32, rect: Rect) -> CanvasClip {
    let lower = |value: f32, limit: u32| math::floor(value).clamp(0.0, limit as _) as u32;
    let upper = |value: f32, limit: u32| math::ceil(value).clamp(0.0, limit as _) as u32;
    let bounds = (lower(rect.left(), width), lower(rect.top(), height),
        upper(rect.right(), width), upper(rect.bottom(), height));
    let (clipped_strips, clipped_runs) = clip_sparse_bounds(
        strips, runs, bounds, |coverage, x, y| (u16::from(coverage) *
            u16::from(rect_pixel_coverage(rect, x, y)) + 127).div_euclid(255) as _);
    if clipped_runs.is_empty() { CanvasClip::Empty } else { CanvasClip::Sparse {
        strips: Rc::new(clipped_strips), runs: Rc::new(clipped_runs), width, height,
    } }
}

fn copy_canvas_mask(mask: CoverageMask<'_>) -> CanvasClip {
    let Some((left, top, right, bottom)) = mask.non_zero_bounds() else {
        return CanvasClip::Empty;
    };
    let stride = right - left;
    let (storage_left, storage_top, _, _) = mask.storage_region();
    let mut data = Vec::with_capacity((stride * (bottom - top)) as usize);
    for y in top..bottom {
        let start = (y - storage_top) as usize * mask.stride() as usize +
            (left - storage_left) as usize;
        data.extend_from_slice(&mask.as_bytes()[start..start + stride as usize]);
    }
    CanvasClip::Path { data: data.into(), width: mask.width(), height: mask.height(),
        left, top, right, bottom, stride }
}

fn sparse_canvas_mask(mask: CoverageMask<'_>) -> Option<CanvasClip> {
    let (strips, runs) = sparse_mask_parts(mask)?;
    Some(CanvasClip::Sparse {
        strips: Rc::new(strips), runs: Rc::new(runs),
        width: mask.width(), height: mask.height(),
    })
}

fn normalize_canvas_clip(clip: &mut CanvasClip) {
    let replacement = {
        let CanvasClip::Path { data, width, height, left, top, right, bottom, stride } = clip
            else { return; };
        let mask = CoverageMask::from_region(data, (*width, *height),
            (*left, *top, *right, *bottom), *stride)
            .expect("owned clip storage is internally valid");
        match mask.kind() {
            MaskKind::Empty => Some(CanvasClip::Empty),
            MaskKind::OpaqueRect(bounds) => Some(CanvasClip::Rect(mask_rect(bounds))),
            MaskKind::Coverage(bounds) if bounds != (*left, *top, *right, *bottom) =>
                sparse_canvas_mask(mask).or_else(|| Some(copy_canvas_mask(mask))),
            MaskKind::Coverage(_) => sparse_canvas_mask(mask),
        }
    };
    if let Some(replacement) = replacement { *clip = replacement; }
}

fn multiply_rect_mask(data: &mut [u8], region: (u32, u32, u32, u32), stride: u32,
    rect: Rect) {
    let (left, top, right, bottom) = region;
    let overlap = |from: f32, to: f32, pixel: u32| {
        (to.min(pixel as f32 + 1.0) - from.max(pixel as f32)).clamp(0.0, 1.0)
    };
    for y in top..bottom {
        let vertical = overlap(rect.top(), rect.bottom(), y);
        for x in left..right {
            let clip = overlap(rect.left(), rect.right(), x) * vertical;
            let offset = (y - top) as usize * stride as usize + (x - left) as usize;
            data[offset] = (data[offset] as f32 * clip + 0.5) as _;
        }
    }
}

fn intersect_dense_clip(current: &CanvasClip, next: &mut CanvasClip) {
    let CanvasClip::Path { data, width, height, left, top, right, bottom, stride } = next
        else { return; };
    match current {
        CanvasClip::None => {}
        CanvasClip::Empty => { *next = CanvasClip::Empty; return; }
        CanvasClip::Rect(rect) => multiply_rect_mask(Rc::make_mut(data),
            (*left, *top, *right, *bottom), *stride, *rect),
        CanvasClip::Path { data: mask, width: mask_width, height: mask_height,
            left: mask_left, top: mask_top, right: mask_right, bottom: mask_bottom,
            stride: mask_stride } => {
            if (*width, *height) != (*mask_width, *mask_height) { return; }
            let data = Rc::make_mut(data);
            for y in *top..*bottom {
                for x in *left..*right {
                    let offset = (y - *top) as usize * *stride as usize +
                        (x - *left) as usize;
                    let mask_value = if x >= *mask_left && x < *mask_right &&
                        y >= *mask_top && y < *mask_bottom {
                        mask[(y - *mask_top) as usize * *mask_stride as usize +
                            (x - *mask_left) as usize]
                    } else { 0 };
                    data[offset] = (data[offset] as u16 * mask_value as u16 + 127)
                        .div_euclid(255) as _;
                }
            }
        }
        CanvasClip::Sparse { strips, runs, width: mask_width, height: mask_height } => {
            if (*width, *height) != (*mask_width, *mask_height) { return; }
            multiply_sparse_mask(Rc::make_mut(data), (*left, *top, *right, *bottom),
                *stride, strips, runs);
        }
    }
    normalize_canvas_clip(next);
}

fn intersect_canvas_clip(current: &CanvasClip, next: &mut CanvasClip) {
    if let CanvasClip::Rect(rect) = next {
        *next = match current {
            CanvasClip::None => return,
            CanvasClip::Empty => CanvasClip::Empty,
            CanvasClip::Rect(current) => intersect_rects(*current, *rect)
                .map_or(CanvasClip::Empty, CanvasClip::Rect),
            CanvasClip::Path { .. } => {
                let mut dense = current.clone();
                if let CanvasClip::Path { data, left, top, right, bottom, stride, .. } = &mut dense {
                    multiply_rect_mask(Rc::make_mut(data),
                        (*left, *top, *right, *bottom), *stride, *rect);
                    normalize_canvas_clip(&mut dense);
                }
                dense
            }
            CanvasClip::Sparse { strips, runs, width, height } =>
                clip_sparse_rect(strips, runs, *width, *height, *rect),
        };
        return;
    }
    if matches!(next, CanvasClip::Empty) { return; }
    if matches!(next, CanvasClip::Path { .. }) {
        intersect_dense_clip(current, next);
        return;
    }
    let replacement = {
        let CanvasClip::Sparse { strips, runs, width, height } = next else { return; };
        match current {
            CanvasClip::None => None,
            CanvasClip::Empty => Some(CanvasClip::Empty),
            CanvasClip::Rect(rect) =>
                Some(clip_sparse_rect(strips, runs, *width, *height, *rect)),
            CanvasClip::Path { width: dense_width, height: dense_height, .. } => {
                if (*width, *height) != (*dense_width, *dense_height) { return; }
                let mut dense = current.clone();
                intersect_dense_clip(next, &mut dense);
                Some(dense)
            }
            CanvasClip::Sparse { strips: current_strips, runs: current_runs,
                width: current_width, height: current_height } => {
                if (*width, *height) != (*current_width, *current_height) { return; }
                let (strips, runs) = intersect_sparse_masks(
                    current_strips, current_runs, strips, runs);
                Some(if runs.is_empty() { CanvasClip::Empty } else { CanvasClip::Sparse {
                    strips: Rc::new(strips), runs: Rc::new(runs),
                    width: *width, height: *height,
                } })
            }
        }
    };
    if let Some(replacement) = replacement { *next = replacement; }
}

impl Canvas<'static> {
    /// Creates a zero-initialized tightly packed RGBA8888 canvas.
    pub fn new(width: u32, height: u32) -> Result<Self, PixmapError> {
        Ok(Self::from_target(Pixmap::new(width, height)?))
    }
}

impl<'target> Canvas<'target> {
    /// Creates a canvas over caller-owned RGBA8888 storage.
    pub fn from_buffer(data: &'target mut [u8], width: u32, height: u32, stride: u32) ->
        Result<Self, PixmapError> {
        Ok(Self::from_target(Pixmap::from_buffer(data, width, height, stride)?))
    }

    fn from_target(target: Pixmap<'target>) -> Self {
        Self {
            target,
            storage: CanvasStorage::default(),
            state: DrawState {
                transform: Affine::identity(), fill_rule: FillRule::NonZero,
                flatten: FlattenOptions::default(), stroke: StrokeOptions::default(),
                paint: SolidPaint::new(SRGBA::black()),
                global_alpha: u8::MAX,
            },
            comp_mode: CompositeMode::SrcOver, clip: CanvasClip::None,
            saved: Vec::new(),
        }
    }

    pub fn target(&self) -> &Pixmap<'target> { &self.target }
    pub fn target_mut(&mut self) -> &mut Pixmap<'target> { &mut self.target }
    pub fn transform(&self) -> Affine { self.state.transform }
    pub fn fill_rule(&self) -> FillRule { self.state.fill_rule }
    pub fn flatten(&self) -> FlattenOptions { self.state.flatten }
    pub fn stroke_options(&self) -> StrokeOptions { self.state.stroke }
    pub fn global_alpha(&self) -> u8 { self.state.global_alpha }
    pub fn composite_mode(&self) -> CompositeMode { self.comp_mode }

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
    /// Sets the global opacity applied to every paint sample (`255` is opaque).
    pub fn set_global_alpha(&mut self, value: u8) -> &mut Self {
        self.state.global_alpha = value; self
    }
    /// Selects the compositing/blending operation used by subsequent draws.
    ///
    /// Color blend functions operate in this encoded-sRGB target's working
    /// space and preserve its premultiplied storage invariant. The mode is part
    /// of the state captured by [`Self::save`] and [`Self::restore`].
    ///
    /// ```
    /// use ugl_rs::{CompositeMode, Canvas};
    ///
    /// let mut canvas = Canvas::new(8, 8).unwrap();
    /// canvas.set_composite_mode(CompositeMode::Multiply).save()
    ///       .set_composite_mode(CompositeMode::Screen);
    /// assert_eq!(canvas.composite_mode(), CompositeMode::Screen);
    /// assert!(canvas.restore());
    /// assert_eq!(canvas.composite_mode(), CompositeMode::Multiply);
    /// ```
    pub fn set_composite_mode(&mut self, value: CompositeMode) -> &mut Self {
        self.comp_mode = value; self
    }

    /// Saves the complete drawing state, including the current clip.
    pub fn save(&mut self) -> &mut Self {
        self.saved.push(SavedCanvasState {
            draw: self.state, comp_mode: self.comp_mode, clip: self.clip.clone(),
        }); self
    }

    /// Restores the most recently saved drawing state.
    ///
    /// Returns `false` without changing state when the stack is empty.
    pub fn restore(&mut self) -> bool {
        let Some(saved) = self.saved.pop() else { return false; };
        self.recycle_clip();
        (self.state, self.comp_mode, self.clip) =
            (saved.draw, saved.comp_mode, saved.clip); true
    }

    fn recycle_clip(&mut self) {
        let clip = core::mem::replace(&mut self.clip, CanvasClip::None);
        if let CanvasClip::Sparse { strips, runs, .. } = clip {
            if let Ok(mut strips) = Rc::try_unwrap(strips) {
                strips.clear(); self.storage.clip_strips = strips;
            }
            if let Ok(mut runs) = Rc::try_unwrap(runs) {
                runs.clear(); self.storage.clip_runs = runs;
            }
        }
    }

    /// Removes the complete accumulated clip.
    pub fn clear_clip(&mut self) -> &mut Self { self.recycle_clip(); self }

    /// Intersects the current clip with `value`.
    pub fn set_clip_rect(&mut self, value: Rect) -> &mut Self {
        if let CanvasClip::Sparse { strips, runs, width, height } = &self.clip {
            self.clip = clip_sparse_rect(strips, runs, *width, *height, value);
            return self;
        }
        if let CanvasClip::Path { data, left, top, right, bottom, stride, .. } = &mut self.clip {
            multiply_rect_mask(Rc::make_mut(data),
                (*left, *top, *right, *bottom), *stride, value);
            normalize_canvas_clip(&mut self.clip);
            return self;
        }
        self.clip = match &self.clip {
            CanvasClip::None => CanvasClip::Rect(value),
            CanvasClip::Empty => CanvasClip::Empty,
            CanvasClip::Rect(current) => intersect_rects(*current, value)
                .map_or(CanvasClip::Empty, CanvasClip::Rect),
            CanvasClip::Path { .. } => unreachable!(),
            CanvasClip::Sparse { .. } => unreachable!(),
        }; self
    }

    /// Intersects the current clip with a retained copy of `value`.
    ///
    /// Dimensions must match the target; validation happens before changing the clip.
    ///
    /// ```
    /// use ugl_rs::{common::{RenderError, raster::CoverageMask}, float::Canvas};
    ///
    /// let pixels = [255];
    /// let mask = CoverageMask::new(&pixels, 1, 1, 1).unwrap();
    /// let mut canvas = Canvas::new(4, 2).unwrap();
    /// assert!(matches!(canvas.set_clip_mask(mask),
    ///     Err(RenderError::CoverageDimensionsMismatch { .. })));
    /// ```
    pub fn set_clip_mask(&mut self, value: CoverageMask<'_>) ->
        Result<&mut Self, RenderError> {
        validate_coverage_dimensions(value.width(), value.height(), &self.target)?;
        match value.kind() {
            MaskKind::Empty => { self.clip = CanvasClip::Empty; return Ok(self); }
            MaskKind::OpaqueRect(bounds) => {
                self.set_clip_rect(mask_rect(bounds)); return Ok(self);
            }
            MaskKind::Coverage(_) => {}
        }
        let mut clip = sparse_canvas_mask(value).unwrap_or_else(|| copy_canvas_mask(value));
        normalize_canvas_clip(&mut clip);
        intersect_canvas_clip(&self.clip, &mut clip);
        self.clip = clip; Ok(self)
    }

    /// Rasterizes an antialiased path and intersects it with the current clip.
    pub fn set_clip_path(&mut self, path: &Path) -> Result<&mut Self, RenderError> {
        self.plan_fill(path)?;
        let (width, height) = (self.target.width(), self.target.height());
        let options = RenderOptions {
            fill_rule: self.state.fill_rule, flatten: self.state.flatten,
        };
        let (edge_count, region) = {
            let mut context_workspace = self.storage.workspace();
            let workspace = render_workspace(&mut context_workspace.stroke);
            let edge_count = build_edges(path, self.state.transform,
                options.flatten, workspace.edges)?;
            (edge_count, edge_region(&workspace.edges[..edge_count], width, height))
        };
        let mut sink = SparseCoverageSink::new(region,
            core::mem::take(&mut self.storage.clip_strips),
            core::mem::take(&mut self.storage.clip_runs));
        if region.0 < region.2 && region.1 < region.3 {
            let mut context_workspace = self.storage.workspace();
            let mut workspace = render_workspace(&mut context_workspace.stroke);
            rasterize_built_region(edge_count, (width, height), region,
                options.fill_rule, &mut sink, &mut workspace)?;
        }
        let mut clip = finish_sparse_clip(
            sink.strips, sink.runs, width, height, &mut self.storage)?;
        intersect_canvas_clip(&self.clip, &mut clip);
        self.clip = clip;
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
            let result = stroke_requirements(
                path, self.state.transform, options,
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

    fn as_canvas_ref(&mut self) -> Result<CanvasRef<'_, 'target, '_, '_>, RenderError> {
        let (state, comp_mode) = (self.state, self.comp_mode);
        let (target, storage, clip) = (&mut self.target, &mut self.storage, &self.clip);
        let mut canvas = CanvasRef::new(target, storage.workspace());
        canvas.state = state;
        canvas.comp_mode = comp_mode;
        canvas.clip = clip.as_clip()?;
        Ok(canvas)
    }

    pub fn fill(&mut self, path: &Path) -> Result<(), RenderError> {
        let paint = self.state.paint; self.fill_with(path, &paint)
    }
    pub fn fill_with<S: PaintSampler>(&mut self, path: &Path, paint: &S) ->
        Result<(), RenderError> {
        self.plan_fill(path)?;
        self.as_canvas_ref()?.fill_with(path, paint)
    }
    pub fn stroke(&mut self, path: &Path) -> Result<(), RenderError> {
        let paint = self.state.paint; self.stroke_with(path, &paint)
    }
    pub fn stroke_with<S: PaintSampler>(&mut self, path: &Path, paint: &S) ->
        Result<(), RenderError> {
        self.plan_stroke(path)?;
        self.as_canvas_ref()?.stroke_with(path, paint)
    }
    pub fn stroke_dashed(&mut self, path: &Path, dash: DashPattern<'_>) ->
        Result<(), RenderError> {
        let paint = self.state.paint; self.stroke_dashed_with(path, &paint, dash)
    }
    pub fn stroke_dashed_with<S: PaintSampler>(&mut self, path: &Path,
        paint: &S, dash: DashPattern<'_>) -> Result<(), RenderError> {
        self.plan_dashed(path, dash)?;
        self.as_canvas_ref()?.stroke_dashed_with(path, paint, dash)
    }
}

#[cfg(test)] mod tests {
    use super::*;
    use crate::{common::{geometry::{Edge, PathBuilder, Point}, raster::CoverageMask,
            render::SpreadMode, stroke::StrokeContour},
        float::{dash::DashPattern,
            sampler::{GradientStop, GradientStops, LinearGradient}},
    };

    struct Buffers {
        points: [Point; 8], contours: [StrokeContour; 2], edges: [Edge; 32],
        dash_points: [Point; 16], dash_contours: [DashContour; 8],
        intersections: [Intersection; 32], cells: [Cell; 4],
        row_offsets: [u32; 5], edge_indices: [u32; 32],
    }

    impl Buffers {
        fn new() -> Self { Self {
            points: [Point::default(); 8], contours: [StrokeContour::default(); 2],
            dash_points: [Point::default(); 16],
            dash_contours: [DashContour::default(); 8],
            edges: [Edge::default(); 32],
            intersections: [Intersection::default(); 32],
            cells: [Cell::default(); 4],
            row_offsets: [0; 5], edge_indices: [0; 32],
        } }

        fn workspace(&mut self) -> Workspace<'_> {
            Workspace::new(StrokeWorkspace {
                    points: &mut self.points, contours: &mut self.contours,
                    edges: &mut self.edges, intersections: &mut self.intersections,
                    cells: &mut self.cells,
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

    #[test] fn owning_canvas_specializes_empty_and_opaque_rectangle_masks() {
        let mut canvas = Canvas::new(4, 2).unwrap();
        let opaque = [0, 255, 255, 0, 0, 255, 255, 0];
        canvas.set_clip_mask(CoverageMask::new(&opaque, 4, 2, 4).unwrap()).unwrap();
        let CanvasClip::Rect(rect) = canvas.clip else { panic!("expected rectangle clip") };
        assert_eq!((rect.left(), rect.top(), rect.right(), rect.bottom()),
            (1.0, 0.0, 3.0, 2.0));

        assert!(matches!(canvas.set_clip_mask(CoverageMask::new(&[255], 1, 1, 1).unwrap()),
            Err(RenderError::CoverageDimensionsMismatch {
                coverage: (1, 1), target: (4, 2),
            })));
        assert!(matches!(canvas.clip, CanvasClip::Rect(_)));

        canvas.set_clip_mask(CoverageMask::new(&[0; 8], 4, 2, 4).unwrap()).unwrap();
        assert!(matches!(canvas.clip, CanvasClip::Empty));

        canvas.clear_clip().set_clip_path(&rectangle()).unwrap();
        assert!(matches!(canvas.clip, CanvasClip::Rect(_)));

        let coverage = [0, 128, 128, 0, 0, 128, 128, 0];
        canvas.clear_clip().set_clip_mask(CoverageMask::new(&coverage, 4, 2, 4).unwrap())
            .unwrap()
            .set_clip_rect(Rect::from_ltrb(3.0, 0.0, 4.0, 2.0).unwrap());
        assert!(matches!(canvas.clip, CanvasClip::Empty));
    }

    #[test] fn owning_canvas_retains_slender_path_clips_sparsely() {
        let mut clip = PathBuilder::new();
        clip.move_to((0.25, 0.0)).line_to((1.25, 0.0))
            .line_to((63.25, 64.0)).line_to((62.25, 64.0));
        let mut canvas = Canvas::new(64, 64).unwrap();
        canvas.set_clip_path(&clip.build()).unwrap();
        let CanvasClip::Sparse { strips, runs, .. } = &canvas.clip
            else { panic!("slender path clip was retained densely") };
        assert_eq!(strips.len(), 4);
        assert!(runs.len() <= 64 * 3);

        canvas.set_clip_rect(Rect::from_ltrb(16.5, 16.5, 48.5, 48.5).unwrap());
        assert!(matches!(canvas.clip, CanvasClip::Sparse { .. }));
    }

    #[test] fn owning_canvas_retains_sparse_external_masks() {
        let mut coverage = alloc::vec![0; 64 * 64];
        for y in 0..64 { coverage[y * 64 + y] = 128; }
        let mut canvas = Canvas::new(64, 64).unwrap();
        canvas.set_clip_mask(CoverageMask::new(&coverage, 64, 64, 64).unwrap()).unwrap();
        let CanvasClip::Sparse { strips, runs, .. } = &canvas.clip
            else { panic!("sparse external mask was retained densely") };
        assert_eq!((strips.len(), runs.len()), (4, 64));
    }

    #[test] fn canvas_manages_fill_stroke_and_dash_scratch_internally() {
        let mut canvas = Canvas::new(4, 4).unwrap();
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
        assert_eq!(canvas.target().as_bytes().len(), 4 * 4 * 4);
    }

    #[test] fn canvas_owns_free_path_clip_storage() {
        let mut clip = PathBuilder::new();
        clip.move_to((0.25, 0.0)).line_to((2.25, 0.0))
            .line_to((2.25, 4.0)).line_to((0.25, 4.0));
        let mut shape = PathBuilder::new();
        shape.move_to((0.0, 0.0)).line_to((4.0, 0.0))
            .line_to((4.0, 4.0)).line_to((0.0, 4.0));
        let mut pixels = [0; 4 * 4 * 4];
        let mut canvas = Canvas::from_buffer(&mut pixels, 4, 4, 16).unwrap();
        canvas.set_color(SRGBA::red()).set_clip_path(&clip.build()).unwrap();
        let CanvasClip::Path { data, left, top, right, bottom, stride, .. } = &canvas.clip
            else { panic!("path clip was not retained as a mask") };
        assert_eq!((*left, *top, *right, *bottom, *stride, data.len()),
            (0, 0, 3, 4, 3, 12));
        canvas.fill(&shape.build()).unwrap();
        assert_ne!(canvas.target().pixel_bytes(0, 1).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(3, 1).unwrap(), [0; 4]);
    }

    #[test] fn canvas_copies_external_clip_masks() {
        let mut canvas = Canvas::new(4, 4).unwrap();
        {
            let data = [255, 255, 0, 0, 255, 255, 0, 0,
                        255, 255, 0, 0, 255, 255, 0, 0];
            canvas.set_clip_mask(CoverageMask::new(&data, 4, 4, 4).unwrap()).unwrap();
        }
        let mut shape = PathBuilder::new();
        shape.move_to((0.0, 0.0)).line_to((4.0, 0.0))
            .line_to((4.0, 4.0)).line_to((0.0, 4.0));
        canvas.set_color(SRGBA::red()).fill(&shape.build()).unwrap();
        assert_ne!(canvas.target().pixel_bytes(0, 1).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(3, 1).unwrap(), [0; 4]);
    }

    #[test] fn canvas_save_restore_preserves_state_and_nested_clip() {
        let mut shape = PathBuilder::new();
        shape.move_to((0.0, 0.0)).line_to((4.0, 0.0))
            .line_to((4.0, 4.0)).line_to((0.0, 4.0));
        let path = shape.build();
        let mut canvas = Canvas::new(4, 4).unwrap();
        let saved_transform = Affine::translate(0.25, 0.0);
        canvas.set_transform(saved_transform)
            .set_global_alpha(192)
            .set_color(SRGBA::red())
            .set_clip_rect(Rect::from_ltrb(0.0, 0.0, 3.0, 4.0).unwrap())
            .save()
            .set_transform(Affine::identity())
            .set_global_alpha(64)
            .set_color(SRGBA::green())
            .set_clip_rect(Rect::from_ltrb(1.0, 0.0, 4.0, 4.0).unwrap());
        canvas.fill(&path).unwrap();
        assert_eq!(canvas.target().pixel_bytes(0, 1).unwrap(), [0; 4]);
        assert_ne!(canvas.target().pixel_bytes(1, 1).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(3, 1).unwrap(), [0; 4]);

        assert!(canvas.restore());
        assert_eq!(canvas.transform(), saved_transform);
        assert_eq!(canvas.global_alpha(), 192);
        assert_eq!(canvas.state.paint, SolidPaint::new(SRGBA::red()));
        assert!(!canvas.restore());
        canvas.fill(&path).unwrap();
        assert_ne!(canvas.target().pixel_bytes(0, 1).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(3, 1).unwrap(), [0; 4]);
    }

    #[test] fn global_alpha_applies_to_solid_and_custom_paint() {
        let mut canvas = Canvas::new(2, 1).unwrap();
        canvas.set_color(SRGBA::red()).set_global_alpha(128)
            .fill(&rectangle()).unwrap();
        assert_eq!(canvas.target().pixel_bytes(0, 0).unwrap(), [128, 0, 0, 128]);

        canvas.target_mut().as_bytes_mut().fill(0);
        let stops = [GradientStop::new(0.0, SRGBA::green()),
                     GradientStop::new(1.0, SRGBA::green())];
        let gradient = LinearGradient::new((0.0, 0.0), (2.0, 0.0),
            GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
        canvas.fill_with(&rectangle(), &gradient).unwrap();
        assert_eq!(canvas.target().pixel_bytes(0, 0).unwrap(), [0, 128, 0, 128]);
    }

    #[test] fn canvas_composite_mode_is_saved_and_composites_in_encoded_target_space() {
        let path = rectangle();
        let mut canvas = Canvas::new(4, 4).unwrap();
        canvas.set_color(SRGBA::blue()).fill(&path).unwrap();
        canvas.set_composite_mode(CompositeMode::Multiply).save()
            .set_composite_mode(CompositeMode::Screen).set_color(SRGBA::red())
            .fill(&path).unwrap();
        assert_eq!(canvas.target().pixel(1, 1).unwrap().to_array(), [255, 0, 255, 255]);
        assert!(canvas.restore());
        assert_eq!(canvas.composite_mode(), CompositeMode::Multiply);
        canvas.set_color(SRGBA::red()).fill(&path).unwrap();
        assert_eq!(canvas.target().pixel(1, 1).unwrap().to_array(), [255, 0, 0, 255]);
    }

    #[test] fn porter_duff_clear_interpolates_antialiased_coverage() {
        let mut shape = PathBuilder::new();
        shape.move_to((0.5, 0.0)).line_to((1.5, 0.0))
            .line_to((1.5, 1.0)).line_to((0.5, 1.0));
        let mut canvas = Canvas::new(2, 1).unwrap();
        canvas.set_color(SRGBA::blue()).fill(&rectangle()).unwrap();
        canvas.set_composite_mode(CompositeMode::Clear).fill(&shape.build()).unwrap();
        for x in 0..2 {
            let [r, g, b, a] = canvas.target().pixel(x, 0).unwrap().to_array();
            assert_eq!((r, g), (0, 0));
            assert!((127..=128).contains(&b));
            assert_eq!(a, b);
        }
    }

    #[test] fn canvas_path_clips_intersect_instead_of_replacing() {
        let rectangle = |left, right| {
            let mut builder = PathBuilder::new();
            builder.move_to((left, 0.0)).line_to((right, 0.0))
                .line_to((right, 4.0)).line_to((left, 4.0));
            builder.build()
        };
        let mut canvas = Canvas::new(4, 4).unwrap();
        canvas.set_clip_path(&rectangle(0.0, 3.0)).unwrap()
            .set_clip_path(&rectangle(1.0, 4.0)).unwrap();
        canvas.set_color(SRGBA::red()).fill(&rectangle(0.0, 4.0)).unwrap();
        assert_eq!(canvas.target().pixel_bytes(0, 1).unwrap(), [0; 4]);
        assert_ne!(canvas.target().pixel_bytes(1, 1).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(3, 1).unwrap(), [0; 4]);
    }

    #[test] fn canvas_ref_fill_state_and_clip_match_low_level_pipeline() {
        let mut pixels = [0; 4 * 4 * 4];
        let mut target = Pixmap::from_buffer(&mut pixels, 4, 4, 16).unwrap();
        let mut buffers = Buffers::new();
        let workspace = buffers.workspace();
        let mask_data = [
            255, 128, 0, 0,
            255, 128, 0, 0,
            0,   0,   0, 0,
            0,   0,   0, 0,
        ];
        let mut context = CanvasRef::new(&mut target, workspace);
        context.set_color(SRGBA::new(255, 0, 0, 128))
            .set_transform(Affine::translate(1.0, 0.0))
            .set_clip_mask(CoverageMask::new(&mask_data, 4, 4, 4).unwrap()).unwrap();
        let mut planning_edges = [Edge::default(); 8];
        let required = context.fill_requirements(&rectangle(), &mut planning_edges).unwrap();
        assert_eq!((required.edges, required.cells), (2, 4));
        context.fill(&rectangle()).unwrap();
        assert_eq!(
            &pixels[..16], &[0, 0, 0, 0, 64, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test] fn canvas_ref_stroke_and_custom_paint_share_current_state() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 1.0)).line_to((4.0, 1.0));
        let path = builder.build();
        let stops = [GradientStop::new(0.0, SRGBA::red()),
                     GradientStop::new(1.0, SRGBA::blue())];
        let gradient = LinearGradient::new((0.0, 0.0), (4.0, 0.0),
            GradientStops::new(&stops).unwrap(), SpreadMode::Pad).unwrap();
        let mut pixels = [0; 4 * 3 * 4];
        let mut target = Pixmap::from_buffer(&mut pixels, 4, 3, 16).unwrap();
        let mut buffers = Buffers::new();
        let workspace = buffers.workspace();
        let mut context = CanvasRef::new(&mut target, workspace);
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

    #[test] fn canvas_ref_dashed_stroke_uses_current_paint_and_clip() {
        let mut builder = PathBuilder::new();
        builder.move_to((0.0, 1.0)).line_to((4.0, 1.0));
        let mut pixels = [0; 4 * 3 * 4];
        let mut target = Pixmap::from_buffer(&mut pixels, 4, 3, 16).unwrap();
        let mut buffers = Buffers::new();
        let workspace = buffers.workspace();
        let mut context = CanvasRef::new(&mut target, workspace);
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
