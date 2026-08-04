//! Stateful facade for the fixed-point rendering pipeline.

use alloc::{rc::Rc, vec::Vec};
use crate::{
    common::{color::SRGBA, dash::DashContour,
        geometry::{Affine, Edge, Path, Point, Rect},
        raster::{CoverageMask, FillRule, MaskKind, SparseCoverageSink, SparseStorage,
            clip_sparse_bounds, finish_sparse_coverage, intersect_sparse_masks,
            multiply_sparse_mask, sparse_mask_parts},
        render::{Clip, DrawState, GlobalAlphaPaint, validate_coverage_dimensions},
        stroke::{StrokeContour, StrokePathWorkspace},
        Pixmap, PixmapError, RenderError, SolidPaint},
    fixed::{DEVICE_RAW_LIMIT, Scalar, canvas::{DashedStrokePathOptions,
            DashedStrokeRequirements, DashedStrokeWorkspace, GeometryWorkspace,
            RenderOptions, RenderRequirements, StrokePathOptions,
            StrokePlanningWorkspace, StrokeRequirements,
            dashed_stroke_requirements as plan_dashed_stroke, render_requirements,
            map_render_error, prepare_dashed_stroke_path, prepare_stroke_path,
            stroke_requirements,
            render_paint, render_paint_clipped, render_paint_masked,
            render_paint_sparse_masked, render_path, render_path_clipped,
            render_path_masked, render_path_sparse_masked},
        dash::Pattern as DashPattern,
        flatten::Options as FlattenOptions,
        raster::{CoverageRun, CoverageStrip, CoverageStrips, Error as RasterError,
            Line, Segment, Trapezoid, Workspace as RasterWorkspace, WorkspaceKind,
            rasterize_lines_region},
        sampler::PaintSampler, stroke::Options as StrokeOptions},
};

/// Caller-owned scratch borrowed by [`CanvasRef`].
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

#[derive(Default)] struct CanvasStorage {
    points: Vec<Point<Scalar>>, contours: Vec<StrokeContour>,
    dash_points: Vec<Point<Scalar>>, dash_contours: Vec<DashContour>,
    edges: Vec<Edge<Scalar>>, lines: Vec<Line>, segments: Vec<Segment>,
    trapezoids: Vec<Trapezoid>, row_area: Vec<u32>, strip_offsets: Vec<u32>,
    strip_indices: Vec<u32>, clip_strips: Vec<CoverageStrip>, clip_runs: Vec<CoverageRun>,
}

impl CanvasStorage {
    fn geometry(&mut self) -> GeometryWorkspace<'_> {
        GeometryWorkspace { edges: &mut self.edges, lines: &mut self.lines }
    }

    fn workspace(&mut self) -> Workspace<'_> {
        Workspace::new(StrokePathWorkspace {
            points: &mut self.points, contours: &mut self.contours,
        }, &mut self.dash_points, &mut self.dash_contours,
        GeometryWorkspace { edges: &mut self.edges, lines: &mut self.lines },
        RasterWorkspace {
            segments: &mut self.segments, trapezoids: &mut self.trapezoids,
            row_area: &mut self.row_area, strip_offsets: &mut self.strip_offsets,
            strip_indices: &mut self.strip_indices,
        })
    }

    fn stroke_planning(&mut self) -> StrokePlanningWorkspace<'_> {
        StrokePlanningWorkspace {
            path: StrokePathWorkspace {
                points: &mut self.points, contours: &mut self.contours,
            },
            geometry: GeometryWorkspace { edges: &mut self.edges, lines: &mut self.lines },
        }
    }

    fn dashed_planning(&mut self) -> DashedStrokeWorkspace<'_> {
        DashedStrokeWorkspace {
            path: StrokePathWorkspace {
                points: &mut self.points, contours: &mut self.contours,
            },
            dash_points: &mut self.dash_points, dash_contours: &mut self.dash_contours,
            geometry: GeometryWorkspace { edges: &mut self.edges, lines: &mut self.lines },
        }
    }

    fn grow_for(&mut self, error: RenderError) -> bool {
        let grow = |len: usize, required: usize| required.max(len.saturating_mul(2).max(8));
        match error {
            RenderError::EdgeCapacity { needed_at_least } =>
                self.edges.resize(grow(self.edges.len(), needed_at_least), Edge::default()),
            RenderError::StrokePointCapacity { needed_at_least } =>
                self.points.resize(grow(self.points.len(), needed_at_least), Point::default()),
            RenderError::StrokeContourCapacity { needed_at_least } => self.contours.resize(
                grow(self.contours.len(), needed_at_least), Default::default()),
            RenderError::DashPointCapacity { needed_at_least } => self.dash_points.resize(
                grow(self.dash_points.len(), needed_at_least), Point::default()),
            RenderError::DashContourCapacity { needed_at_least } => self.dash_contours.resize(
                grow(self.dash_contours.len(), needed_at_least), Default::default()),
            RenderError::FixedRaster(RasterError::WorkspaceTooSmall {
                kind: WorkspaceKind::Lines, required,
            }) => self.lines.resize(grow(self.lines.len(), required), Line::default()),
            _ => return false,
        }
        true
    }

    fn prepare(&mut self, required: RenderRequirements) {
        self.edges.resize(required.edges, Edge::default());
        self.lines.resize(required.lines, Line::default());
        self.segments.resize(required.segments, Segment::default());
        self.trapezoids.resize(required.trapezoids, Trapezoid::default());
        self.row_area.resize(required.row_area, 0);
        self.strip_offsets.resize(required.strip_offsets, 0);
        self.strip_indices.resize(required.strip_indices, 0);
    }
}

#[derive(Clone)] enum CanvasClip {
    None, Empty, Rect(Rect<Scalar>),
    Path { data: Rc<[u8]>, width: u32, height: u32,
        left: u32, top: u32, right: u32, bottom: u32, stride: u32 },
    Sparse { strips: Rc<Vec<CoverageStrip>>, runs: Rc<Vec<CoverageRun>>,
        width: u32, height: u32 },
}

impl CanvasClip {
    fn as_clip(&self) -> Result<Clip<'_, Scalar>, RenderError> { Ok(match self {
        Self::None => Clip::None,
        Self::Empty => Clip::Rect(Rect::from_ltrb(
            Scalar::ZERO, Scalar::ZERO, Scalar::ZERO, Scalar::ZERO)
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

fn intersect_rects(a: Rect<Scalar>, b: Rect<Scalar>) -> Option<Rect<Scalar>> {
    let (left, top) = (a.left().max(b.left()), a.top().max(b.top()));
    let (right, bottom) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    (left < right && top < bottom).then(||
        Rect::from_ltrb(left, top, right, bottom).expect("ordered rectangle intersection"))
}

fn mask_rect((left, top, right, bottom): (u32, u32, u32, u32)) -> Option<Rect<Scalar>> {
    let scalar = |value: u32| {
        (value <= DEVICE_RAW_LIMIT as u32 >> 8)
            .then(|| Scalar::from_bits((value << 8) as _))
    };
    Rect::from_ltrb(scalar(left)?, scalar(top)?, scalar(right)?, scalar(bottom)?)
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
            MaskKind::OpaqueRect(bounds) => mask_rect(bounds).map(CanvasClip::Rect),
            MaskKind::Coverage(bounds) if bounds != (*left, *top, *right, *bottom) =>
                sparse_canvas_mask(mask).or_else(|| Some(copy_canvas_mask(mask))),
            MaskKind::Coverage(_) => sparse_canvas_mask(mask),
        }
    };
    if let Some(replacement) = replacement { *clip = replacement; }
}

fn multiply_rect_mask(data: &mut [u8], region: (u32, u32, u32, u32), stride: u32,
    rect: Rect<Scalar>) {
    let (left, top, right, bottom) = region;
    for y in top..bottom {
        for x in left..right {
            let offset = (y - top) as usize * stride as usize + (x - left) as usize;
            let area = rect_pixel_area(rect, x, y);
            data[offset] = ((u32::from(data[offset]) * area + 32_768) / 65_536) as _;
        }
    }
}

fn rect_pixel_area(rect: Rect<Scalar>, x: u32, y: u32) -> u32 {
    const SCALE: i64 = 256;
    let overlap = |from: Scalar, to: Scalar, pixel: u32| {
        let pixel = i64::from(pixel) * SCALE;
        (i64::from(to.to_bits()).min(pixel + SCALE) -
         i64::from(from.to_bits()).max(pixel)).clamp(0, SCALE) as u32
    };
    overlap(rect.left(), rect.right(), x) * overlap(rect.top(), rect.bottom(), y)
}

fn clip_sparse_rect(strips: &[CoverageStrip], runs: &[CoverageRun],
    width: u32, height: u32, rect: Rect<Scalar>) -> CanvasClip {
    const SCALE: i64 = 256;
    let lower = |value: Scalar, limit: u32| i64::from(value.to_bits()).div_euclid(SCALE)
        .clamp(0, i64::from(limit)) as u32;
    let upper = |value: Scalar, limit: u32| {
        let raw = i64::from(value.to_bits());
        (raw.div_euclid(SCALE) + (raw.rem_euclid(SCALE) != 0) as i64)
            .clamp(0, i64::from(limit)) as u32
    };
    let bounds = (lower(rect.left(), width), lower(rect.top(), height),
        upper(rect.right(), width), upper(rect.bottom(), height));
    let (clipped_strips, clipped_runs) = clip_sparse_bounds(
        strips, runs, bounds, |coverage, x, y|
            ((u32::from(coverage) * rect_pixel_area(rect, x, y) + 32_768) / 65_536) as _);
    if clipped_runs.is_empty() { CanvasClip::Empty } else { CanvasClip::Sparse {
        strips: Rc::new(clipped_strips), runs: Rc::new(clipped_runs), width, height,
    } }
}

fn finish_sparse_clip(strips: Vec<CoverageStrip>, runs: Vec<CoverageRun>,
    width: u32, height: u32, storage: &mut CanvasStorage) ->
    Result<CanvasClip, RenderError> {
    Ok(match finish_sparse_coverage(strips, runs, width, height,
        &mut storage.clip_strips, &mut storage.clip_runs)
        .ok_or(RenderError::DimensionsOverflow)? {
        SparseStorage::Empty => CanvasClip::Empty,
        SparseStorage::OpaqueRect(bounds) => mask_rect(bounds)
            .map_or(CanvasClip::Empty, CanvasClip::Rect),
        SparseStorage::Sparse { strips, runs } => CanvasClip::Sparse {
            strips: Rc::new(strips), runs: Rc::new(runs), width, height,
        },
        SparseStorage::Dense { data, left, top, right, bottom, stride } =>
            CanvasClip::Path {
                data: data.into(), width, height, left, top, right, bottom, stride,
            },
    })
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
            for y in *top..*bottom { for x in *left..*right {
                let offset = (y - *top) as usize * *stride as usize +
                    (x - *left) as usize;
                let mask_value = if x >= *mask_left && x < *mask_right &&
                    y >= *mask_top && y < *mask_bottom {
                    mask[(y - *mask_top) as usize * *mask_stride as usize +
                        (x - *mask_left) as usize]
                } else { 0 };
                data[offset] = (u16::from(data[offset]) * u16::from(mask_value) + 127)
                    .div_euclid(255) as _;
            } }
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

fn edge_region(edges: &[Edge<Scalar>], width: u32, height: u32) ->
    (u32, u32, u32, u32) {
    const SCALE: i64 = 256;
    let Some(first) = edges.first() else { return (0, 0, 0, 0); };
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (
        first.upper.x.to_bits(), first.upper.y.to_bits(),
        first.upper.x.to_bits(), first.upper.y.to_bits());
    for edge in edges { for point in [edge.upper, edge.lower] {
        min_x = min_x.min(point.x.to_bits()); min_y = min_y.min(point.y.to_bits());
        max_x = max_x.max(point.x.to_bits()); max_y = max_y.max(point.y.to_bits());
    } }
    let lower = |raw: i32, maximum: u32| i64::from(raw).div_euclid(SCALE)
        .clamp(0, i64::from(maximum)) as u32;
    let upper = |raw: i32, maximum: u32| (i64::from(raw) + SCALE - 1)
        .div_euclid(SCALE).clamp(0, i64::from(maximum)) as u32;
    (lower(min_x, width), lower(min_y, height),
     upper(max_x, width), upper(max_y, height))
}

#[derive(Clone)] struct SavedCanvasState {
    draw: DrawState<Scalar, FlattenOptions, StrokeOptions, SolidPaint>, clip: CanvasClip,
}

/// Stateful Q24.8 drawing facade.
///
/// Methods accepting [`PaintSampler`] use fixed-point geometry and coverage.
pub struct CanvasRef<'a, 'target, 'workspace, 'clip> {
    target: &'a mut Pixmap<'target>,
    workspace: Workspace<'workspace>,
    state: DrawState<Scalar, FlattenOptions, StrokeOptions, SolidPaint>,
    clip: Clip<'clip, Scalar>,
}

impl<'a, 'target, 'workspace, 'clip> CanvasRef<'a, 'target, 'workspace, 'clip> {
    pub fn new(target: &'a mut Pixmap<'target>,
        workspace: Workspace<'workspace>) -> Self {
        Self {
            target, workspace,
            state: DrawState {
                transform: Affine::identity(), fill_rule: FillRule::NonZero,
                flatten: FlattenOptions::default(),
                stroke: StrokeOptions::default(),
                paint: SolidPaint::new(SRGBA::black()),
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
        self.state.paint = SolidPaint::new(color); self
    }

    /// Sets the global opacity applied with integer arithmetic (`255` is opaque).
    pub fn set_global_alpha(&mut self, alpha: u8) -> &mut Self {
        self.state.global_alpha = alpha; self
    }

    pub fn clear_clip(&mut self) -> &mut Self { self.clip = Clip::None; self }

    pub fn set_clip_rect(&mut self, rect: Rect<Scalar>) -> &mut Self {
        self.clip = Clip::Rect(rect); self
    }

    pub fn set_clip_mask(&mut self, mask: CoverageMask<'clip>) ->
        Result<&mut Self, RenderError> {
        validate_coverage_dimensions(mask.width(), mask.height(), self.target)?;
        self.clip = Clip::Mask(mask); Ok(self)
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
            Clip::SparseMask(mask) => render_path_sparse_masked(path, &paint, mask, options,
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
        stroke_requirements(path, StrokePathOptions {
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
            Clip::SparseMask(mask) => render_paint_sparse_masked(
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
            Clip::SparseMask(mask) => render_paint_sparse_masked(
                lines, &paint, mask, FillRule::NonZero, self.target, &mut workspace.raster),
        }
    }
}

/// Convenient fixed-point renderer with owned target state and automatically
/// managed scratch storage.
///
/// After capacity warm-up, draw calls reuse their allocations. Use [`CanvasRef`]
/// when every scratch buffer must remain caller-owned or statically bounded.
pub struct Canvas<'target> {
    target: Pixmap<'target>, storage: CanvasStorage,
    state: DrawState<Scalar, FlattenOptions, StrokeOptions, SolidPaint>,
    clip: CanvasClip, saved: Vec<SavedCanvasState>,
}

impl Canvas<'static> {
    pub fn new(width: u32, height: u32) -> Result<Self, PixmapError> {
        Ok(Self::from_target(Pixmap::new(width, height)?))
    }
}

impl<'target> Canvas<'target> {
    pub fn from_buffer(data: &'target mut [u8], width: u32, height: u32, stride: u32) ->
        Result<Self, PixmapError> {
        Ok(Self::from_target(Pixmap::from_buffer(data, width, height, stride)?))
    }

    fn from_target(target: Pixmap<'target>) -> Self {
        Self {
            target, storage: CanvasStorage::default(),
            state: DrawState {
                transform: Affine::identity(), fill_rule: FillRule::NonZero,
                flatten: FlattenOptions::default(), stroke: StrokeOptions::default(),
                paint: SolidPaint::new(SRGBA::black()), global_alpha: u8::MAX,
            },
            clip: CanvasClip::None, saved: Vec::new(),
        }
    }

    pub fn target(&self) -> &Pixmap<'target> { &self.target }
    pub fn target_mut(&mut self) -> &mut Pixmap<'target> { &mut self.target }
    pub fn transform(&self) -> Affine<Scalar> { self.state.transform }
    pub fn fill_rule(&self) -> FillRule { self.state.fill_rule }
    pub fn flatten(&self) -> FlattenOptions { self.state.flatten }
    pub fn stroke_options(&self) -> StrokeOptions { self.state.stroke }
    pub fn global_alpha(&self) -> u8 { self.state.global_alpha }

    pub fn set_transform(&mut self, value: Affine<Scalar>) -> &mut Self {
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
    pub fn set_global_alpha(&mut self, value: u8) -> &mut Self {
        self.state.global_alpha = value; self
    }

    pub fn save(&mut self) -> &mut Self {
        self.saved.push(SavedCanvasState { draw: self.state, clip: self.clip.clone() }); self
    }
    pub fn restore(&mut self) -> bool {
        let Some(saved) = self.saved.pop() else { return false; };
        self.recycle_clip();
        (self.state, self.clip) = (saved.draw, saved.clip); true
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
    pub fn clear_clip(&mut self) -> &mut Self { self.recycle_clip(); self }
    pub fn set_clip_rect(&mut self, value: Rect<Scalar>) -> &mut Self {
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
    /// use ugl_rs::{common::{RenderError, raster::CoverageMask}, fixed::Canvas};
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
            MaskKind::OpaqueRect(bounds) => if let Some(rect) = mask_rect(bounds) {
                self.set_clip_rect(rect); return Ok(self);
            },
            MaskKind::Coverage(_) => {}
        }
        let mut clip = sparse_canvas_mask(value).unwrap_or_else(|| copy_canvas_mask(value));
        normalize_canvas_clip(&mut clip);
        intersect_canvas_clip(&self.clip, &mut clip);
        self.clip = clip; Ok(self)
    }

    /// Rasterizes an antialiased Q24.8 path and intersects it with the current clip.
    pub fn set_clip_path(&mut self, path: &Path<Scalar>) -> Result<&mut Self, RenderError> {
        self.plan_fill(path)?;
        let (width, height) = (self.target.width(), self.target.height());
        let region = edge_region(&self.storage.edges, width, height);
        let mut sink = SparseCoverageSink::new(region,
            core::mem::take(&mut self.storage.clip_strips),
            core::mem::take(&mut self.storage.clip_runs));
        if region.0 < region.2 && region.1 < region.3 {
            let mut workspace = self.storage.workspace();
            rasterize_lines_region(workspace.geometry.lines, width, height, region,
                self.state.fill_rule, &mut workspace.raster, &mut sink)
                .map_err(map_render_error)?;
        }
        let mut clip = finish_sparse_clip(
            sink.strips, sink.runs, width, height, &mut self.storage)?;
        intersect_canvas_clip(&self.clip, &mut clip);
        self.clip = clip;
        Ok(self)
    }

    pub fn fill(&mut self, path: &Path<Scalar>) -> Result<(), RenderError> {
        self.plan_fill(path)?;
        let clip = self.clip.as_clip()?;
        let workspace = self.storage.workspace();
        let mut canvas = CanvasRef::new(&mut self.target, workspace);
        (canvas.state, canvas.clip) = (self.state, clip);
        canvas.fill(path)
    }

    pub fn fill_with<S: PaintSampler>(&mut self, path: &Path<Scalar>, paint: &S) ->
        Result<(), RenderError> {
        self.plan_fill(path)?;
        let clip = self.clip.as_clip()?;
        let workspace = self.storage.workspace();
        let mut canvas = CanvasRef::new(&mut self.target, workspace);
        (canvas.state, canvas.clip) = (self.state, clip);
        canvas.fill_with(path, paint)
    }

    pub fn stroke(&mut self, path: &Path<Scalar>) -> Result<(), RenderError> {
        let paint = self.state.paint;
        self.stroke_with(path, &paint)
    }

    pub fn stroke_with<S: PaintSampler>(&mut self, path: &Path<Scalar>, paint: &S) ->
        Result<(), RenderError> {
        self.plan_stroke(path)?;
        let clip = self.clip.as_clip()?;
        let workspace = self.storage.workspace();
        let mut canvas = CanvasRef::new(&mut self.target, workspace);
        (canvas.state, canvas.clip) = (self.state, clip);
        canvas.stroke_with(path, paint)
    }

    pub fn stroke_dashed(&mut self, path: &Path<Scalar>, dash: DashPattern<'_>) ->
        Result<(), RenderError> {
        let paint = self.state.paint;
        self.stroke_dashed_with(path, &paint, dash)
    }

    pub fn stroke_dashed_with<S: PaintSampler>(&mut self, path: &Path<Scalar>, paint: &S,
        dash: DashPattern<'_>) -> Result<(), RenderError> {
        self.plan_dashed_stroke(path, dash)?;
        let clip = self.clip.as_clip()?;
        let workspace = self.storage.workspace();
        let mut canvas = CanvasRef::new(&mut self.target, workspace);
        (canvas.state, canvas.clip) = (self.state, clip);
        canvas.stroke_dashed_with(path, paint, dash)
    }

    fn plan_fill(&mut self, path: &Path<Scalar>) -> Result<(), RenderError> {
        let options = RenderOptions { transform: self.state.transform,
            flatten: self.state.flatten, fill_rule: self.state.fill_rule };
        loop {
            let result = render_requirements(path, options,
                (self.target.width(), self.target.height()), &mut self.storage.geometry());
            match result {
                Ok(required) => { self.storage.prepare(required); return Ok(()); }
                Err(error) if self.storage.grow_for(error) => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn plan_stroke(&mut self, path: &Path<Scalar>) -> Result<(), RenderError> {
        let options = StrokePathOptions { transform: self.state.transform,
            flatten: self.state.flatten, stroke: self.state.stroke };
        loop {
            let result = stroke_requirements(path, options,
                (self.target.width(), self.target.height()),
                &mut self.storage.stroke_planning());
            match result {
                Ok(required) => {
                    self.storage.prepare(required.render);
                    self.storage.points.resize(required.points, Point::default());
                    self.storage.contours.resize(required.contours, Default::default());
                    return Ok(());
                }
                Err(error) if self.storage.grow_for(error) => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn plan_dashed_stroke(&mut self, path: &Path<Scalar>, dash: DashPattern<'_>) ->
        Result<(), RenderError> {
        let options = DashedStrokePathOptions { path: StrokePathOptions {
            transform: self.state.transform, flatten: self.state.flatten,
            stroke: self.state.stroke,
        }, dash };
        loop {
            let result = plan_dashed_stroke(path, options,
                (self.target.width(), self.target.height()),
                &mut self.storage.dashed_planning());
            match result {
                Ok(required) => {
                    self.storage.prepare(required.stroke.render);
                    self.storage.points.resize(required.stroke.points, Point::default());
                    self.storage.contours.resize(required.stroke.contours, Default::default());
                    self.storage.dash_points.resize(required.dash_points, Point::default());
                    self.storage.dash_contours.resize(required.dash_contours, Default::default());
                    return Ok(());
                }
                Err(error) if self.storage.grow_for(error) => {}
                Err(error) => return Err(error),
            }
        }
    }
}

#[cfg(test)] mod tests {
    use super::*;
    use crate::{common::{geometry::{Edge, PathBuilder},
            stroke::{StrokeContour, StrokePathWorkspace}},
        fixed::raster::{Line, Segment, Trapezoid}};

    #[test] fn owning_canvas_specializes_empty_and_opaque_rectangle_masks() {
        let mut canvas = Canvas::new(4, 2).unwrap();
        let opaque = [0, 255, 255, 0, 0, 255, 255, 0];
        canvas.set_clip_mask(CoverageMask::new(&opaque, 4, 2, 4).unwrap()).unwrap();
        let CanvasClip::Rect(rect) = canvas.clip else { panic!("expected rectangle clip") };
        assert_eq!((rect.left(), rect.top(), rect.right(), rect.bottom()),
            (Scalar::from_num(1), Scalar::ZERO, Scalar::from_num(3), Scalar::from_num(2)));

        assert!(matches!(canvas.set_clip_mask(CoverageMask::new(&[255], 1, 1, 1).unwrap()),
            Err(RenderError::CoverageDimensionsMismatch {
                coverage: (1, 1), target: (4, 2),
            })));
        assert!(matches!(canvas.clip, CanvasClip::Rect(_)));

        canvas.set_clip_mask(CoverageMask::new(&[0; 8], 4, 2, 4).unwrap()).unwrap();
        assert!(matches!(canvas.clip, CanvasClip::Empty));

        let mut path = PathBuilder::new();
        path.move_to((Scalar::ZERO, Scalar::ZERO))
            .line_to((Scalar::from_num(2), Scalar::ZERO))
            .line_to((Scalar::from_num(2), Scalar::from_num(2)))
            .line_to((Scalar::ZERO, Scalar::from_num(2)));
        canvas.clear_clip().set_clip_path(&path.build()).unwrap();
        assert!(matches!(canvas.clip, CanvasClip::Rect(_)));

        let (coverage, fixed) = ([0, 128, 128, 0, 0, 128, 128, 0], Scalar::from_num);
        canvas.clear_clip().set_clip_mask(CoverageMask::new(&coverage, 4, 2, 4).unwrap())
            .unwrap()
            .set_clip_rect(Rect::from_ltrb(fixed(3), fixed(0), fixed(4), fixed(2)).unwrap());
        assert!(matches!(canvas.clip, CanvasClip::Empty));
    }

    #[test] fn owning_canvas_retains_sparse_masks_and_restores_them_after_rect_clip() {
        let mut coverage = alloc::vec![0; 64 * 64];
        for y in 0..64 { coverage[y * 64 + y] = 128; }
        let mut canvas = Canvas::new(64, 64).unwrap();
        canvas.set_clip_mask(CoverageMask::new(&coverage, 64, 64, 64).unwrap()).unwrap();
        let CanvasClip::Sparse { strips, runs, .. } = &canvas.clip
            else { panic!("sparse mask was retained densely") };
        assert_eq!((strips.len(), runs.len()), (4, 64));

        canvas.set_clip_rect(Rect::from_ltrb(Scalar::from_num(16.5), Scalar::from_num(16.5),
            Scalar::from_num(47.5), Scalar::from_num(47.5)).unwrap());
        let CanvasClip::Sparse { strips, runs, .. } = &canvas.clip
            else { panic!("rectangle intersection did not restore sparse storage") };
        assert_eq!((strips.len(), runs.len()), (2, 32));

        let mut shape = PathBuilder::new();
        shape.move_to((Scalar::ZERO, Scalar::ZERO))
            .line_to((Scalar::from_num(64), Scalar::ZERO))
            .line_to((Scalar::from_num(64), Scalar::from_num(64)))
            .line_to((Scalar::ZERO, Scalar::from_num(64)));
        canvas.set_color(SRGBA::red()).fill(&shape.build()).unwrap();
        assert_eq!(canvas.target().pixel_bytes(15, 15).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(16, 16).unwrap(), [32, 0, 0, 32]);
        assert_eq!(canvas.target().pixel_bytes(17, 16).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(48, 48).unwrap(), [0; 4]);
    }

    #[test] fn sparse_rectangle_intersection_matches_dense_pixel_arithmetic() {
        let mut coverage = alloc::vec![0; 64 * 64];
        let mut random = 0x1234_5678_u32;
        for _ in 0..128 {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let index = (random as usize >> 8) % coverage.len();
            coverage[index] = (random >> 24) as u8 | 1;
        }
        let rect = Rect::from_ltrb(Scalar::from_num(7.25), Scalar::from_num(9.5),
            Scalar::from_num(53.75), Scalar::from_num(57.25)).unwrap();
        let mut canvas = Canvas::new(64, 64).unwrap();
        canvas.set_clip_mask(CoverageMask::new(&coverage, 64, 64, 64).unwrap()).unwrap()
            .set_clip_rect(rect);
        assert!(matches!(canvas.clip, CanvasClip::Sparse { .. }));

        let mut shape = PathBuilder::new();
        shape.move_to((Scalar::ZERO, Scalar::ZERO))
            .line_to((Scalar::from_num(64), Scalar::ZERO))
            .line_to((Scalar::from_num(64), Scalar::from_num(64)))
            .line_to((Scalar::ZERO, Scalar::from_num(64)));
        canvas.set_color(SRGBA::red()).fill(&shape.build()).unwrap();
        for y in 0..64 { for x in 0..64 {
            let source = coverage[y as usize * 64 + x as usize];
            let expected = ((u32::from(source) * rect_pixel_area(rect, x, y) + 32_768) /
                65_536) as u8;
            assert_eq!(canvas.target().pixel_bytes(x, y).unwrap(),
                [expected, 0, 0, expected]);
        } }
    }

    #[test] fn sparse_and_dense_mask_intersection_matches_scalar_multiplication() {
        let (mut sparse, mut dense) = (alloc::vec![0; 64 * 64], alloc::vec![0; 64 * 64]);
        for y in 0..64_usize {
            sparse[y * 64 + y] = 64 + (y & 127) as u8;
            for x in 0..64 { dense[y * 64 + x] = 32 + ((x * 3 + y * 5) & 127) as u8; }
        }
        let mut canvas = Canvas::new(64, 64).unwrap();
        canvas.set_clip_mask(CoverageMask::new(&sparse, 64, 64, 64).unwrap()).unwrap()
            .set_clip_mask(CoverageMask::new(&dense, 64, 64, 64).unwrap()).unwrap();
        assert!(matches!(canvas.clip, CanvasClip::Sparse { .. }));

        let mut shape = PathBuilder::new();
        shape.move_to((Scalar::ZERO, Scalar::ZERO))
            .line_to((Scalar::from_num(64), Scalar::ZERO))
            .line_to((Scalar::from_num(64), Scalar::from_num(64)))
            .line_to((Scalar::ZERO, Scalar::from_num(64)));
        canvas.set_color(SRGBA::red()).fill(&shape.build()).unwrap();
        for y in 0..64 { for x in 0..64 {
            let index = y as usize * 64 + x as usize;
            let expected = (u16::from(sparse[index]) * u16::from(dense[index]) + 127)
                .div_euclid(255) as u8;
            assert_eq!(canvas.target().pixel_bytes(x, y).unwrap(),
                [expected, 0, 0, expected]);
        } }
    }

    #[test] fn sparse_merge_join_and_restore_match_independent_masks() {
        let (mut left, mut right) = (alloc::vec![0; 64 * 64], alloc::vec![0; 64 * 64]);
        for y in 0..64_usize {
            let x = y * 17 % 64;
            left[y * 64 + x] = 64 + y as u8;
            right[y * 64 + x] = 192 - y as u8;
            right[y * 64 + (x + 5) % 64] = 127;
        }
        let mut shape = PathBuilder::new();
        shape.move_to((Scalar::ZERO, Scalar::ZERO))
            .line_to((Scalar::from_num(64), Scalar::ZERO))
            .line_to((Scalar::from_num(64), Scalar::from_num(64)))
            .line_to((Scalar::ZERO, Scalar::from_num(64)));
        let shape = shape.build();
        let mut canvas = Canvas::new(64, 64).unwrap();
        canvas.set_clip_mask(CoverageMask::new(&left, 64, 64, 64).unwrap()).unwrap().save()
            .set_clip_mask(CoverageMask::new(&right, 64, 64, 64).unwrap()).unwrap()
            .set_color(SRGBA::red()).fill(&shape).unwrap();
        assert!(matches!(canvas.clip, CanvasClip::Sparse { .. }));
        for y in 0..64 { for x in 0..64 {
            let index = y as usize * 64 + x as usize;
            let expected = (u16::from(left[index]) * u16::from(right[index]) + 127)
                .div_euclid(255) as u8;
            assert_eq!(canvas.target().pixel_bytes(x, y).unwrap(),
                [expected, 0, 0, expected]);
        } }

        assert!(canvas.restore());
        canvas.target_mut().as_bytes_mut().fill(0);
        canvas.fill(&shape).unwrap();
        for y in 0..64 { for x in 0..64 {
            let expected = left[y as usize * 64 + x as usize];
            assert_eq!(canvas.target().pixel_bytes(x, y).unwrap(),
                [0, 0, 0, expected]);
        } }
    }

    #[test] fn nested_paths_match_independent_mask_multiplication() {
        let path = |offset: i32| {
            let fixed = Scalar::from_num;
            let mut builder = PathBuilder::new();
            builder.move_to((fixed(4 + offset), fixed(5)))
                .line_to((fixed(58), fixed(13 + offset)))
                .line_to((fixed(37 - offset), fixed(59)))
                .line_to((fixed(7), fixed(43 - offset)));
            builder.build()
        };
        let (left, right) = (path(0), path(6));
        let fixed = Scalar::from_num;
        let mut full = PathBuilder::new();
        full.move_to((fixed(0), fixed(0))).line_to((fixed(64), fixed(0)))
            .line_to((fixed(64), fixed(64))).line_to((fixed(0), fixed(64)));
        let full = full.build();
        let render = |first: &Path<Scalar>, second: Option<&Path<Scalar>>| {
            let mut canvas = Canvas::new(64, 64).unwrap();
            canvas.set_clip_path(first).unwrap();
            if let Some(second) = second { canvas.set_clip_path(second).unwrap(); }
            canvas.set_color(SRGBA::red()).fill(&full).unwrap();
            canvas.target().as_bytes().to_vec()
        };
        let (left_mask, right_mask, combined) = (
            render(&left, None), render(&right, None), render(&left, Some(&right)));
        for ((left, right), combined) in left_mask.chunks_exact(4)
            .zip(right_mask.chunks_exact(4)).zip(combined.chunks_exact(4)) {
            let expected = (u16::from(left[3]) * u16::from(right[3]) + 127)
                .div_euclid(255) as u8;
            assert_eq!(combined, &[expected, 0, 0, expected]);
        }
    }

    #[test] fn randomized_clip_state_sequences_match_a_dense_reference() {
        const WIDTH: usize = 24;
        const HEIGHT: usize = 20;
        let fixed = Scalar::from_bits;
        let full_path = {
            let mut path = PathBuilder::new();
            path.move_to((Scalar::ZERO, Scalar::ZERO))
                .line_to((Scalar::from_num(WIDTH), Scalar::ZERO))
                .line_to((Scalar::from_num(WIDTH), Scalar::from_num(HEIGHT)))
                .line_to((Scalar::ZERO, Scalar::from_num(HEIGHT)));
            path.build()
        };
        let paths = [
            [(384, 256), (5632, 640), (4736, 4736), (768, 4224)],
            [(128, 2432), (2688, 128), (6016, 2816), (2944, 4992)],
            [(1024, 512), (5504, 1536), (4096, 4864), (256, 3072)],
        ].map(|points| {
            let mut path = PathBuilder::new();
            path.move_to((fixed(points[0].0), fixed(points[0].1)));
            for point in &points[1..] { path.line_to((fixed(point.0), fixed(point.1))); }
            path.build()
        });
        let path_masks = paths.each_ref().map(|path| {
            let mut canvas = Canvas::new(WIDTH as _, HEIGHT as _).unwrap();
            canvas.set_clip_path(path).unwrap().set_color(SRGBA::red())
                .fill(&full_path).unwrap();
            canvas.target().as_bytes().chunks_exact(4).map(|pixel| pixel[3])
                .collect::<Vec<_>>()
        });

        let mut masks = [const { [0; WIDTH * HEIGHT] }; 4];
        for y in 3..17 { for x in 4..21 { masks[1][y * WIDTH + x] = u8::MAX; } }
        for y in 0..HEIGHT {
            masks[2][y * WIDTH + y * 7 % WIDTH] = 32 + (y * 11) as u8;
            for x in 0..WIDTH {
                masks[3][y * WIDTH + x] = 1 + ((x * 29 + y * 47) % 254) as u8;
            }
        }
        let rects = [
            Rect::from_ltrb(fixed(128), fixed(192), fixed(6016), fixed(4928)).unwrap(),
            Rect::from_ltrb(fixed(1728), fixed(896), fixed(4544), fixed(4032)).unwrap(),
            Rect::from_ltrb(fixed(-384), fixed(2688), fixed(3328), fixed(5504)).unwrap(),
        ];
        let opaque_mask_rect = Rect::from_ltrb(
            Scalar::from_num(4), Scalar::from_num(3),
            Scalar::from_num(21), Scalar::from_num(17)).unwrap();

        #[derive(Clone)] enum ReferenceClip { None, Rect(Rect<Scalar>), Dense(Vec<u8>) }
        let apply_rect = |clip: &mut ReferenceClip, rect| match clip {
            ReferenceClip::None => *clip = ReferenceClip::Rect(rect),
            ReferenceClip::Rect(current) => *clip = intersect_rects(*current, rect)
                .map_or_else(|| ReferenceClip::Dense(alloc::vec![0; WIDTH * HEIGHT]),
                    ReferenceClip::Rect),
            ReferenceClip::Dense(values) => for y in 0..HEIGHT { for x in 0..WIDTH {
                let value = &mut values[y * WIDTH + x];
                *value = ((u32::from(*value) * rect_pixel_area(rect, x as _, y as _) +
                    32_768) / 65_536) as _;
            } },
        };
        let apply_dense = |clip: &mut ReferenceClip, mask: &[u8]| match clip {
            ReferenceClip::None => *clip = ReferenceClip::Dense(mask.to_vec()),
            ReferenceClip::Rect(rect) => {
                let rect = *rect;
                *clip = ReferenceClip::Dense(mask.iter().enumerate().map(|(index, value)| {
                    let (x, y) = (index % WIDTH, index / WIDTH);
                    ((u32::from(*value) * rect_pixel_area(rect, x as _, y as _) +
                        32_768) / 65_536) as _
                }).collect());
            }
            ReferenceClip::Dense(values) => for (value, mask) in values.iter_mut().zip(mask) {
                *value = (u16::from(*value) * u16::from(*mask) + 127)
                    .div_euclid(255) as _;
            },
        };

        for seed in 1..=8_u32 {
            let mut canvas = Canvas::new(WIDTH as _, HEIGHT as _).unwrap();
            canvas.set_color(SRGBA::red());
            let (mut reference, mut saved) = (ReferenceClip::None, Vec::new());
            let mut random = seed.wrapping_mul(0x9e37_79b9);
            for step in 0..256 {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let choice = (random >> 27) % 8;
                match choice {
                    0 => {
                        canvas.clear_clip(); reference = ReferenceClip::None;
                    }
                    1 => if saved.len() < 12 {
                        canvas.save(); saved.push(reference.clone());
                    },
                    2 => {
                        let expected = saved.pop();
                        assert_eq!(canvas.restore(), expected.is_some());
                        if let Some(value) = expected { reference = value; }
                    }
                    3 | 4 => {
                        let index = (random as usize >> 8) % masks.len();
                        let mask = &masks[index];
                        canvas.set_clip_mask(CoverageMask::new(
                            mask, WIDTH as _, HEIGHT as _, WIDTH as _).unwrap()).unwrap();
                        if index == 1 { apply_rect(&mut reference, opaque_mask_rect); }
                        else { apply_dense(&mut reference, mask); }
                    }
                    5 | 6 => {
                        let rect = rects[(random as usize >> 8) % rects.len()];
                        canvas.set_clip_rect(rect);
                        apply_rect(&mut reference, rect);
                    }
                    _ => {
                        let index = (random as usize >> 8) % paths.len();
                        canvas.set_clip_path(&paths[index]).unwrap();
                        apply_dense(&mut reference, &path_masks[index]);
                    }
                }
                canvas.target_mut().as_bytes_mut().fill(0);
                canvas.fill(&full_path).unwrap();
                for (index, pixel) in canvas.target().as_bytes().chunks_exact(4).enumerate() {
                    let expected = match &reference {
                        ReferenceClip::None => u8::MAX,
                        ReferenceClip::Rect(rect) => ((255 * rect_pixel_area(*rect,
                            (index % WIDTH) as _, (index / WIDTH) as _) + 32_768) /
                            65_536) as _,
                        ReferenceClip::Dense(values) => values[index],
                    };
                    assert_eq!(pixel, &[expected, 0, 0, expected],
                        "seed={seed}, step={step}, pixel={index}, choice={choice}");
                }
            }
        }
    }

    #[test] fn canvas_ref_matches_state_clip_and_workspace_shape() {
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
        let mut context = CanvasRef::new(&mut target, workspace);
        context.set_color(SRGBA::new(255, 0, 0, 128))
            .set_transform(Affine::translate(fixed(1), Scalar::ZERO))
            .set_clip_mask(CoverageMask::new(&mask_data, 4, 3, 4).unwrap()).unwrap();
        context.fill(&path).unwrap();
        assert_eq!(
            &pixels[..16], &[0, 0, 0, 0, 64, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test] fn owning_canvas_grows_scratch_and_retains_state() {
        let fixed = Scalar::from_num;
        let mut builder = PathBuilder::new();
        builder.move_to((fixed(0), fixed(0))).line_to((fixed(2), fixed(0)))
            .line_to((fixed(2), fixed(2))).line_to((fixed(0), fixed(2)));
        let mut pixels = [0; 4 * 3 * 4];
        let mut canvas = Canvas::from_buffer(&mut pixels, 4, 3, 16).unwrap();
        canvas.set_color(SRGBA::new(255, 0, 0, 128)).save()
            .set_global_alpha(128);
        canvas.fill(&builder.build()).unwrap();
        assert_eq!(canvas.global_alpha(), 128);
        assert!(canvas.restore());
        assert_eq!(canvas.global_alpha(), 255);
        assert_eq!(&pixels[..8], &[64, 0, 0, 64, 64, 0, 0, 64]);
    }

    fn rectangle(left: i32, right: i32) -> Path<Scalar> {
        let fixed = Scalar::from_num;
        let mut builder = PathBuilder::new();
        builder.move_to((fixed(left), fixed(0))).line_to((fixed(right), fixed(0)))
            .line_to((fixed(right), fixed(4))).line_to((fixed(left), fixed(4)));
        builder.build()
    }

    #[test] fn owning_canvas_retains_compact_intersecting_path_clips() {
        let fractional_rectangle = |left: i32, right: i32| {
            let (left, right) = (Scalar::from_bits(left), Scalar::from_bits(right));
            let mut builder = PathBuilder::new();
            builder.move_to((left, Scalar::ZERO)).line_to((right, Scalar::ZERO))
                .line_to((right, Scalar::from_num(4)))
                .line_to((left, Scalar::from_num(4)));
            builder.build()
        };
        let mut canvas = Canvas::new(4, 4).unwrap();
        canvas.set_clip_path(&fractional_rectangle(128, 896)).unwrap()
            .set_clip_path(&fractional_rectangle(384, 1024)).unwrap();
        let CanvasClip::Path { data, left, top, right, bottom, stride, .. } = &canvas.clip
            else { panic!("path clip was not retained as a local mask") };
        assert_eq!((*left, *top, *right, *bottom, *stride, data.len()),
            (1, 0, 4, 4, 3, 12));
        canvas.set_color(SRGBA::red()).fill(&rectangle(0, 4)).unwrap();
        assert_eq!(canvas.target().pixel_bytes(0, 1).unwrap(), [0; 4]);
        assert_ne!(canvas.target().pixel_bytes(1, 1).unwrap(), [0; 4]);
        assert_ne!(canvas.target().pixel_bytes(3, 1).unwrap(), [0; 4]);
    }

    #[test] fn owning_canvas_restores_path_clip_with_drawing_state() {
        let mut canvas = Canvas::new(4, 4).unwrap();
        canvas.set_clip_path(&rectangle(0, 2)).unwrap().save()
            .set_clip_path(&rectangle(1, 4)).unwrap();
        assert!(canvas.restore());
        canvas.set_color(SRGBA::red()).fill(&rectangle(0, 4)).unwrap();
        assert_ne!(canvas.target().pixel_bytes(0, 1).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(2, 1).unwrap(), [0; 4]);
    }
}
