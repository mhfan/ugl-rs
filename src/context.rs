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
struct DrawState<T, F, S> {
    transform: Affine<T>, fill_rule: FillRule, flatten: F, stroke: S,
    paint: SolidPaint,
}

/// Stateful analytic f32 drawing facade.
///
/// The context borrows both target and scratch storage. It allocates nothing,
/// and every draw call has the same capacity and error behavior as the
/// corresponding low-level function in [`crate::canvas`].
pub struct Context<'a, 'target, 'workspace, 'clip> {
    target: &'a mut PixmapMut<'target>,
    workspace: &'a mut AnalyticStrokeWorkspace<'workspace>,
    state: DrawState<f32, FlattenOptions, StrokeOptions>,
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

#[cfg(test)] #[path = "context_tests.rs"] mod tests;
