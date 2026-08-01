//! Stateful facade for the fixed-point rendering pipeline.

use alloc::{rc::Rc, vec::Vec};
use crate::{
    common::{color::SRGBA, dash::DashContour,
        geometry::{Affine, Edge, Path, Point, Rect},
        raster::{CoverageMask, FillRule, RegionMaskSink},
        render::{Clip, DrawState, GlobalAlphaPaint},
        stroke::{StrokeContour, StrokePathWorkspace},
        Pixmap, PixmapError, RenderError, SolidPaint},
    fixed::{Scalar, canvas::{DashedStrokePathOptions,
            DashedStrokeRequirements, DashedStrokeWorkspace, GeometryWorkspace,
            RenderOptions, RenderRequirements, StrokePathOptions,
            StrokePlanningWorkspace, StrokeRequirements,
            dashed_stroke_requirements as plan_dashed_stroke, render_requirements,
            map_render_error, prepare_dashed_stroke_path, prepare_stroke_path,
            stroke_requirements,
            render_paint, render_paint_clipped, render_paint_masked,
            render_path, render_path_clipped, render_path_masked},
        dash::Pattern as DashPattern,
        flatten::Options as FlattenOptions, raster::{Error as RasterError, Line, Segment,
            Trapezoid, Workspace as RasterWorkspace, WorkspaceKind,
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
    trapezoids: Vec<Trapezoid>, row_area: Vec<u64>, strip_offsets: Vec<u32>,
    strip_indices: Vec<u32>,
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
    }) }
}

fn intersect_rects(a: Rect<Scalar>, b: Rect<Scalar>) -> Option<Rect<Scalar>> {
    let (left, top) = (a.left().max(b.left()), a.top().max(b.top()));
    let (right, bottom) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    (left < right && top < bottom).then(||
        Rect::from_ltrb(left, top, right, bottom).expect("ordered rectangle intersection"))
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

fn multiply_rect_mask(data: &mut [u8], region: (u32, u32, u32, u32), stride: u32,
    rect: Rect<Scalar>) {
    const SCALE: i64 = 256;
    let overlap = |from: Scalar, to: Scalar, pixel: u32| {
        let pixel = i64::from(pixel) * SCALE;
        (i64::from(to.to_bits()).min(pixel + SCALE) -
         i64::from(from.to_bits()).max(pixel)).clamp(0, SCALE) as u64
    };
    let (left, top, right, bottom) = region;
    for y in top..bottom {
        let vertical = overlap(rect.top(), rect.bottom(), y);
        for x in left..right {
            let offset = (y - top) as usize * stride as usize + (x - left) as usize;
            let area = overlap(rect.left(), rect.right(), x) * vertical;
            data[offset] = ((u64::from(data[offset]) * area + 32_768) / 65_536) as _;
        }
    }
}

fn intersect_canvas_clip(current: &CanvasClip, next: &mut CanvasClip) {
    let CanvasClip::Path { data, width, height, left, top, right, bottom, stride } = next
        else { return; };
    match current {
        CanvasClip::None => {}
        CanvasClip::Empty => Rc::make_mut(data).fill(0),
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
    }
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
        (self.state, self.clip) = (saved.draw, saved.clip); true
    }
    pub fn clear_clip(&mut self) -> &mut Self { self.clip = CanvasClip::None; self }
    pub fn set_clip_rect(&mut self, value: Rect<Scalar>) -> &mut Self {
        self.clip = match &mut self.clip {
            CanvasClip::None => CanvasClip::Rect(value),
            CanvasClip::Empty => CanvasClip::Empty,
            CanvasClip::Rect(current) => intersect_rects(*current, value)
                .map_or(CanvasClip::Empty, CanvasClip::Rect),
            CanvasClip::Path { data, left, top, right, bottom, stride, .. } => {
                multiply_rect_mask(Rc::make_mut(data),
                    (*left, *top, *right, *bottom), *stride, value);
                return self;
            }
        }; self
    }
    pub fn set_clip_mask(&mut self, value: CoverageMask<'_>) -> &mut Self {
        let mut clip = copy_canvas_mask(value);
        intersect_canvas_clip(&self.clip, &mut clip);
        self.clip = clip; self
    }

    /// Rasterizes an antialiased Q24.8 path and intersects it with the current clip.
    pub fn set_clip_path(&mut self, path: &Path<Scalar>) -> Result<&mut Self, RenderError> {
        self.plan_fill(path)?;
        let (width, height) = (self.target.width(), self.target.height());
        let region = edge_region(&self.storage.edges, width, height);
        let (left, top, right, bottom) = region;
        let region_width = right - left;
        let length = usize::try_from(region_width).ok().and_then(|width|
            usize::try_from(bottom - top).ok().and_then(|height| width.checked_mul(height)))
            .ok_or(RenderError::DimensionsOverflow)?;
        let mut data = alloc::vec![0; length];
        if length != 0 {
            let mut sink = RegionMaskSink::new(&mut data, region);
            let mut workspace = self.storage.workspace();
            rasterize_lines_region(workspace.geometry.lines, width, height, region,
                self.state.fill_rule, &mut workspace.raster, &mut sink)
                .map_err(map_render_error)?;
        }
        let mask = CoverageMask::from_region(&data, (width, height), region, region_width)
            .map_err(|_| RenderError::DimensionsOverflow)?;
        let mut clip = if mask.non_zero_bounds() == Some(region) {
            CanvasClip::Path { data: data.into(), width, height, left, top,
                right, bottom, stride: region_width }
        } else { copy_canvas_mask(mask) };
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
            .set_clip_mask(CoverageMask::new(&mask_data, 4, 3, 4).unwrap());
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
        let mut canvas = Canvas::new(4, 4).unwrap();
        canvas.set_clip_path(&rectangle(0, 3)).unwrap()
            .set_clip_path(&rectangle(1, 4)).unwrap();
        let CanvasClip::Path { data, left, top, right, bottom, stride, .. } = &canvas.clip
            else { panic!("path clip was not retained as a local mask") };
        assert_eq!((*left, *top, *right, *bottom, *stride, data.len()),
            (1, 0, 4, 4, 3, 12));
        canvas.set_color(SRGBA::red()).fill(&rectangle(0, 4)).unwrap();
        assert_eq!(canvas.target().pixel_bytes(0, 1).unwrap(), [0; 4]);
        assert_ne!(canvas.target().pixel_bytes(1, 1).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(3, 1).unwrap(), [0; 4]);
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
