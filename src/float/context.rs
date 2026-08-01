//! Stateful drawing facades over the allocation-free rendering pipelines.

use alloc::{rc::Rc, vec::Vec};
use core::convert::Infallible;
use crate::{
    common::{Pixmap, RenderError, SolidPaint},
    analytic::Intersection as AnalyticIntersection,
    canvas::{DashedStrokePathOptions, DashedStrokePlanningWorkspace,
        DashedStrokeRequirements, DashedStrokeWorkspace, RenderOptions,
        RenderRequirements, RenderWorkspace, StrokePathOptions, StrokePlanningWorkspace,
        StrokeRequirements, StrokeWorkspace,
        build_edges, dashed_stroke_requirements as plan_dashed_stroke, edge_region,
        rasterize_built_region,
        render_paint, render_requirements,
        render_paint_clipped, render_paint_masked,
        render_stroke_paint_dashed, render_stroke_paint_dashed_clipped,
        render_stroke_paint_dashed_masked,
        render_stroke_paint, render_stroke_paint_clipped,
        render_stroke_paint_masked},
    color::SRGBA, dash::DashContour, edge::Edge, flatten::FlattenOptions,
    float::{dash::DashPattern, stroke::StrokeOptions},
    geometry::{Affine, Path, Point, Rect},
    raster::{CoverageMask, CoverageSink, FillRule}, sampler::PaintSampler,
    render::{Clip, DrawState, GlobalAlphaPaint},
    stroke::StrokeContour,
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
/// corresponding low-level function in [`crate::canvas`].
pub struct CanvasRef<'a, 'target, 'workspace, 'clip> {
    target: &'a mut Pixmap<'target>,
    workspace: Workspace<'workspace>,
    state: DrawState<f32, FlattenOptions, StrokeOptions, SolidPaint>,
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
            },
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
        let (target, paint) = (&mut *self.target,
            GlobalAlphaPaint::new(paint, self.state.global_alpha));
        let mut workspace = render_workspace(&mut self.workspace.stroke);
        match clip {
            Clip::None => render_paint(
                path, transform, &paint, options, target, &mut workspace),
            Clip::Rect(rect) => render_paint_clipped(
                path, transform, &paint, rect, options, target, &mut workspace),
            Clip::Mask(mask) => render_paint_masked(
                path, transform, &paint, mask, options, target, &mut workspace),
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
        let (target, paint) = (&mut *self.target,
            GlobalAlphaPaint::new(paint, self.state.global_alpha));
        match clip {
            Clip::None => render_stroke_paint(
                path, transform, &paint, options, target, &mut self.workspace.stroke),
            Clip::Rect(rect) => render_stroke_paint_clipped(
                path, transform, &paint, rect, options, target, &mut self.workspace.stroke),
            Clip::Mask(mask) => render_stroke_paint_masked(
                path, transform, &paint, mask, options, target, &mut self.workspace.stroke),
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
        let (target, paint) = (&mut *self.target,
            GlobalAlphaPaint::new(paint, self.state.global_alpha));
        let workspace = &mut self.workspace;
        let mut dashed = DashedStrokeWorkspace {
            stroke: reborrow_stroke(&mut workspace.stroke),
            dash_points: workspace.dash_points,
            dash_contours: workspace.dash_contours,
        };
        match clip {
            Clip::None => render_stroke_paint_dashed(
                path, transform, &paint, options, target, &mut dashed),
            Clip::Rect(rect) => render_stroke_paint_dashed_clipped(
                path, transform, &paint, rect, options, target, &mut dashed),
            Clip::Mask(mask) => render_stroke_paint_dashed_masked(
                path, transform, &paint, mask, options, target, &mut dashed),
        }
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
    intersections: Vec<AnalyticIntersection>, cells: Vec<crate::analytic::Cell>,
    row_offsets: Vec<u32>, edge_indices: Vec<u32>,
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
        self.intersections.resize(required.intersections, AnalyticIntersection::default());
        self.cells.resize(required.cells, crate::analytic::Cell::default());
        self.row_offsets.resize(required.row_offsets, 0);
        self.edge_indices.resize(required.edge_indices, 0);
    }
}

/// Convenient stateful f32 renderer with automatically managed scratch storage.
///
/// `Canvas` plans and grows scratch before every draw, then delegates to the
/// allocation-free [`CanvasRef`]. Geometry or capacity failure therefore occurs
/// before the destination is modified. Use `CanvasRef` or [`crate::canvas`]
/// directly when scratch must be statically supplied.
pub struct Canvas<'target> {
    target: Pixmap<'target>, storage: CanvasStorage,
    state: DrawState<f32, FlattenOptions, StrokeOptions, SolidPaint>,
    clip: CanvasClip, saved: Vec<SavedCanvasState>,
}

#[derive(Clone)] enum CanvasClip {
    None, Empty, Rect(Rect),
    Path { data: Rc<[u8]>, width: u32, height: u32,
        left: u32, top: u32, right: u32, bottom: u32, stride: u32 },
}

#[derive(Clone)] struct SavedCanvasState {
    draw: DrawState<f32, FlattenOptions, StrokeOptions, SolidPaint>, clip: CanvasClip,
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
    }) }
}

fn intersect_rects(a: Rect, b: Rect) -> Option<Rect> {
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

fn intersect_canvas_clip(current: &CanvasClip, next: &mut CanvasClip) {
    let CanvasClip::Path { data, width, height, left, top, right, bottom, stride } = next
        else { return; };
    match current {
        CanvasClip::None => {}
        CanvasClip::Empty => {
            let data = Rc::make_mut(data);
            data.fill(0);
        }
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
    }
}

struct ClipMaskSink<'a> {
    data: &'a mut [u8], left: u32, top: u32, width: u32, height: u32,
}

impl CoverageSink for ClipMaskSink<'_> {
    type Error = Infallible;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        if x < self.left || y < self.top || y >= self.top + self.height {
            return Ok(());
        }
        let start_x = x - self.left;
        if start_x >= self.width { return Ok(()); }
        let len = len.min(self.width - start_x);
        let start = (y - self.top) as usize * self.width as usize + start_x as usize;
        self.data[start..start + len as usize].fill(coverage);
        Ok(())
    }
}

impl Canvas<'static> {
    /// Creates a zero-initialized tightly packed RGBA8888 canvas.
    pub fn new(width: u32, height: u32) -> Result<Self, crate::PixmapError> {
        Ok(Self::from_target(Pixmap::new(width, height)?))
    }
}

impl<'target> Canvas<'target> {
    /// Creates a canvas over caller-owned RGBA8888 storage.
    pub fn from_buffer(data: &'target mut [u8], width: u32, height: u32, stride: u32) ->
        Result<Self, crate::PixmapError> {
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
            clip: CanvasClip::None,
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

    /// Saves the complete drawing state, including the current clip.
    pub fn save(&mut self) -> &mut Self {
        self.saved.push(SavedCanvasState { draw: self.state, clip: self.clip.clone() }); self
    }

    /// Restores the most recently saved drawing state.
    ///
    /// Returns `false` without changing state when the stack is empty.
    pub fn restore(&mut self) -> bool {
        let Some(saved) = self.saved.pop() else { return false; };
        (self.state, self.clip) = (saved.draw, saved.clip); true
    }

    /// Removes the complete accumulated clip.
    pub fn clear_clip(&mut self) -> &mut Self { self.clip = CanvasClip::None; self }

    /// Intersects the current clip with `value`.
    pub fn set_clip_rect(&mut self, value: Rect) -> &mut Self {
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

    /// Intersects the current clip with a retained copy of `value`.
    pub fn set_clip_mask(&mut self, value: CoverageMask<'_>) -> &mut Self {
        let mut clip = copy_canvas_mask(value);
        intersect_canvas_clip(&self.clip, &mut clip);
        self.clip = clip; self
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
        let (left, top, right, bottom) = region;
        let (region_width, region_height) = (right - left, bottom - top);
        let length = usize::try_from(region_width).ok().and_then(|width|
            usize::try_from(region_height).ok().and_then(|height| width.checked_mul(height)))
            .ok_or(RenderError::DimensionsOverflow)?;
        let mut data = alloc::vec![0; length];
        if length != 0 {
            let mut sink = ClipMaskSink { data: &mut data, left, top,
                width: region_width, height: region_height };
            let mut context_workspace = self.storage.workspace();
            let mut workspace = render_workspace(&mut context_workspace.stroke);
            rasterize_built_region(edge_count, (width, height), region,
                options.fill_rule, &mut sink, &mut workspace)?;
        }
        let mask = CoverageMask::from_region(&data, (width, height), region, region_width)
            .map_err(|_| RenderError::DimensionsOverflow)?;
        let mut clip = if let Some((mask_left, mask_top, mask_right, mask_bottom)) =
            mask.non_zero_bounds() {
            if (mask_left, mask_top, mask_right, mask_bottom) == region {
                CanvasClip::Path { data: data.into(), width, height, left, top,
                    right, bottom, stride: region_width }
            } else {
                copy_canvas_mask(mask)
            }
        } else { CanvasClip::Empty };
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

    fn as_canvas_ref(&mut self) -> Result<CanvasRef<'_, 'target, '_, '_>, RenderError> {
        let state = self.state;
        let (target, storage, clip) = (&mut self.target, &mut self.storage, &self.clip);
        let mut canvas = CanvasRef::new(target, storage.workspace());
        canvas.state = state;
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
    use crate::{analytic::Intersection as AnalyticIntersection,
        edge::Edge, float::dash::DashPattern, geometry::{PathBuilder, Point},
        raster::CoverageMask,
        sampler::{GradientStop, GradientStops, LinearGradient}, render::SpreadMode,
        stroke::StrokeContour,
    };

    struct Buffers {
        points: [Point; 8], contours: [StrokeContour; 2], edges: [Edge; 32],
        dash_points: [Point; 16], dash_contours: [DashContour; 8],
        intersections: [AnalyticIntersection; 32], cells: [crate::analytic::Cell; 4],
        row_offsets: [u32; 5], edge_indices: [u32; 32],
    }

    impl Buffers {
        fn new() -> Self { Self {
            points: [Point::default(); 8], contours: [StrokeContour::default(); 2],
            dash_points: [Point::default(); 16],
            dash_contours: [DashContour::default(); 8],
            edges: [Edge::default(); 32],
            intersections: [AnalyticIntersection::default(); 32],
            cells: [crate::analytic::Cell::default(); 4],
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
        clip.move_to((0.0, 0.0)).line_to((2.0, 0.0))
            .line_to((2.0, 4.0)).line_to((0.0, 4.0));
        let mut shape = PathBuilder::new();
        shape.move_to((0.0, 0.0)).line_to((4.0, 0.0))
            .line_to((4.0, 4.0)).line_to((0.0, 4.0));
        let mut pixels = [0; 4 * 4 * 4];
        let mut canvas = Canvas::from_buffer(&mut pixels, 4, 4, 16).unwrap();
        canvas.set_color(SRGBA::red()).set_clip_path(&clip.build()).unwrap();
        let CanvasClip::Path { data, left, top, right, bottom, stride, .. } = &canvas.clip
            else { panic!("path clip was not retained as a mask") };
        assert_eq!((*left, *top, *right, *bottom, *stride, data.len()),
            (0, 0, 2, 4, 2, 8));
        canvas.fill(&shape.build()).unwrap();
        assert_ne!(canvas.target().pixel_bytes(0, 1).unwrap(), [0; 4]);
        assert_eq!(canvas.target().pixel_bytes(3, 1).unwrap(), [0; 4]);
    }

    #[test] fn canvas_copies_external_clip_masks() {
        let mut canvas = Canvas::new(4, 4).unwrap();
        {
            let data = [255, 255, 0, 0, 255, 255, 0, 0,
                        255, 255, 0, 0, 255, 255, 0, 0];
            canvas.set_clip_mask(CoverageMask::new(&data, 4, 4, 4).unwrap());
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
            .set_clip_mask(CoverageMask::new(&mask_data, 4, 4, 4).unwrap());
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
