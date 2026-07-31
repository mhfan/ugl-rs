//! Workspace and option types for fixed-point rendering.

use crate::{
    dash::{DashContour, FixedDashPattern},
    edge::Edge, fixed::{flatten::FixedFlattenOptions, raster::FixedLine,
        stroke::FixedStrokeOptions},
    geometry::{Affine, FixedScalar, Point}, raster::FillRule,
    stroke::StrokePathWorkspace,
};

pub struct FixedGeometryWorkspace<'a> {
    pub edges: &'a mut [Edge<FixedScalar>],
    pub lines: &'a mut [FixedLine],
}

pub struct FixedDashedStrokeWorkspace<'a> {
    pub path: StrokePathWorkspace<'a, FixedScalar>,
    pub dash_points: &'a mut [Point<FixedScalar>],
    pub dash_contours: &'a mut [DashContour],
    pub geometry: FixedGeometryWorkspace<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq)] pub struct FixedRenderOptions {
    pub transform: Affine<FixedScalar>,
    pub flatten: FixedFlattenOptions,
    pub fill_rule: FillRule,
}

impl Default for FixedRenderOptions { fn default() -> Self {
    Self { transform: Affine::identity(), flatten: FixedFlattenOptions::default(),
        fill_rule: FillRule::NonZero }
} }

#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct FixedStrokePathOptions {
    pub transform: Affine<FixedScalar>,
    pub flatten: FixedFlattenOptions,
    pub stroke: FixedStrokeOptions,
}

#[derive(Clone, Copy, Debug)] pub struct FixedDashedStrokePathOptions<'a> {
    pub path: FixedStrokePathOptions,
    pub dash: FixedDashPattern<'a>,
}
