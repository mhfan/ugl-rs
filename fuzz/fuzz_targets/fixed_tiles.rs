#![no_main]

use core::convert::Infallible;
use libfuzzer_sys::fuzz_target;
use ugl_rs::{analytic::{Intersection as FloatIntersection, Workspace as FloatWorkspace,
        rasterize_edges as rasterize_float_edges},
    common::geometry::Edge, fixed::{Scalar, raster::{
        STRIP_HEIGHT, CoverageRun, CoverageStrip, CoverageWorkspace,
        Error, Line, Workspace, RenderError, Segment, Trapezoid, WorkspaceKind,
        strip_requirements, prepare_lines, rasterize_lines, rasterize_lines_to_strips,
    },
    tile::{CoverageTile, CoverageTileRun, DirectTilePiece,
        DirectTileWorkspace, requirements as tile_requirements, rasterize_lines_to_tiles}},
    geometry::Point, raster::FillRule,
};

fn point(bytes: [u8; 4], width: u32, height: u32) -> Point<Scalar> {
    let coordinate = |bytes: [u8; 2], extent: u32| {
        let range = (extent + 8) * 256;
        Scalar::from_bits(u16::from_le_bytes(bytes) as i32 % range as i32 - 1024)
    };
    (coordinate([bytes[0], bytes[1]], width),
     coordinate([bytes[2], bytes[3]], height)).into()
}

fn edge(from: Point<Scalar>, to: Point<Scalar>) -> Option<Edge<Scalar>> {
    if from.y < to.y {
        Some(Edge { upper: from, lower: to, winding: 1 })
    } else if to.y < from.y {
        Some(Edge { upper: to, lower: from, winding: -1 })
    } else { None }
}

fuzz_target!(|data: &[u8]| {
    let Some((&width, rest)) = data.split_first() else { return; };
    let Some((&height, mut data)) = rest.split_first() else { return; };
    let (width, height) = (1 + width as u32 % 48, 1 + height as u32 % 48);
    let mut edges = Vec::new();
    while let Some((&encoded_count, rest)) = data.split_first() {
        let count = 3 + encoded_count as usize % 6;
        data = rest;
        if data.len() < count * 4 { break; }
        let points = data[..count * 4].chunks(4).map(|bytes|
            point(bytes.try_into().unwrap(), width, height)).collect::<Vec<_>>();
        data = &data[count * 4..];
        for index in 0..count {
            if let Some(edge) = edge(points[index], points[(index + 1) % count]) {
                edges.push(edge);
            }
        }
    }

    if !edges.is_empty() {
        let mut insufficient = vec![Line::default(); edges.len() - 1];
        let before = insufficient.clone();
        assert_eq!(prepare_lines(&edges, &mut insufficient), Err(Error::WorkspaceTooSmall {
            kind: WorkspaceKind::Lines, required: edges.len(),
        }));
        assert_eq!(insufficient, before);
    }
    let mut lines = vec![Line::default(); edges.len()];
    let line_count = prepare_lines(&edges, &mut lines).unwrap();
    lines.truncate(line_count);
    let float_edges = edges.iter().map(|edge| Edge {
        upper: (edge.upper.x.to_num::<f32>(), edge.upper.y.to_num::<f32>()).into(),
        lower: (edge.lower.x.to_num::<f32>(), edge.lower.y.to_num::<f32>()).into(),
        winding: edge.winding,
    }).collect::<Vec<_>>();
    let strip_requirements = strip_requirements(&lines, height).unwrap();
    let tile_requirements = tile_requirements(width, height).unwrap();
    let (mut segments, mut trapezoids, mut row_area) = (
        vec![Segment::default(); lines.len()],
        vec![Trapezoid::default(); lines.len().div_ceil(2)],
        vec![0; width as usize],
    );
    let (mut offsets, mut indices) = (
        vec![0; strip_requirements.offsets], vec![0; strip_requirements.indices],
    );
    let (mut strips, mut strip_runs) = (
        vec![CoverageStrip::default(); height.div_ceil(STRIP_HEIGHT) as usize],
        vec![CoverageRun::default(); width as usize * height as usize],
    );
    let (mut tiles, mut tile_runs, mut pieces) = (
        vec![CoverageTile::default(); tile_requirements.tiles],
        vec![CoverageTileRun::default(); tile_requirements.runs],
        vec![DirectTilePiece::default(); tile_requirements.pieces],
    );
    let (mut heads, mut tails, mut touched) = (
        vec![0; tile_requirements.columns], vec![0; tile_requirements.columns],
        vec![0; tile_requirements.columns],
    );

    for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
        let mut workspace = Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut offsets, strip_indices: &mut indices,
        };
        let mut streamed = vec![0; width as usize * height as usize];
        let result = rasterize_lines(&lines, width, height, fill_rule, &mut workspace,
            &mut |x, y, coverage| {
                streamed[y as usize * width as usize + x as usize] = coverage;
                Ok::<_, Infallible>(())
            });
        if let Err(error) = result {
            let error = match error {
                RenderError::Raster(error) => error,
                RenderError::Sink(never) => match never {},
            };
            assert_eq!(rasterize_lines_to_strips(&lines, width, height, fill_rule,
                &mut workspace, CoverageWorkspace {
                    strips: &mut strips, runs: &mut strip_runs,
                }).unwrap_err(), error);
            assert_eq!(rasterize_lines_to_tiles(&lines, width, height, fill_rule,
                &mut workspace, DirectTileWorkspace {
                    tiles: &mut tiles, runs: &mut tile_runs, pieces: &mut pieces,
                    column_heads: &mut heads, column_tails: &mut tails,
                    touched_columns: &mut touched,
                }).unwrap_err(), error);
            continue;
        }

        let (mut float_pixels, mut float_row, mut float_intersections) = (
            vec![0; streamed.len()], vec![0.0; width as usize],
            vec![FloatIntersection::default(); float_edges.len()],
        );
        rasterize_float_edges(&float_edges, width, height, fill_rule,
            &mut FloatWorkspace {
                intersections: &mut float_intersections, row_coverage: &mut float_row,
            }, &mut |x, y, coverage| {
                float_pixels[y as usize * width as usize + x as usize] = coverage;
                Ok::<_, Infallible>(())
            }).unwrap();
        // A constant per-pixel error is meaningful for a simple triangle.
        // Arbitrary self-intersecting multi-contour inputs can accumulate
        // independently rounded fixed crossings, so their stronger oracle is
        // the exact stream/strip/tile equivalence checked below.
        if edges.len() <= 3 {
            for (&fixed, &float) in streamed.iter().zip(&float_pixels) {
                assert!(fixed.abs_diff(float) <= 2,
                    "fixed={fixed}, float={float}, size={width}x{height}, \
                     rule={fill_rule:?}");
            }
        }

        let retained = rasterize_lines_to_strips(&lines, width, height, fill_rule,
            &mut workspace, CoverageWorkspace {
                strips: &mut strips, runs: &mut strip_runs,
            }).unwrap();
        let mut replayed = vec![0; streamed.len()];
        retained.replay(&mut |x, y, coverage| {
            replayed[y as usize * width as usize + x as usize] = coverage;
            Ok::<_, Infallible>(())
        }).unwrap();
        assert_eq!(replayed, streamed);

        let tiled = rasterize_lines_to_tiles(&lines, width, height, fill_rule,
            &mut workspace, DirectTileWorkspace {
                tiles: &mut tiles, runs: &mut tile_runs, pieces: &mut pieces,
                column_heads: &mut heads, column_tails: &mut tails,
                touched_columns: &mut touched,
            }).unwrap();
        replayed.fill(0);
        tiled.replay(&mut |x, y, coverage| {
            replayed[y as usize * width as usize + x as usize] = coverage;
            Ok::<_, Infallible>(())
        }).unwrap();
        assert_eq!(replayed, streamed);
    }
});
