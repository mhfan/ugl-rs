use super::*;
use alloc::{vec, vec::Vec};
use core::convert::Infallible;
use crate::{
    edge::Edge,
    geometry::FixedScalar,
    raster::FillRule,
    raster_fixed::{
        FixedCoverageRun, FixedCoverageStrip, FixedCoverageWorkspace, FixedLine,
        FixedRasterWorkspace, FixedSegment, FixedTrapezoid, fixed_strip_requirements,
        prepare_lines, rasterize_lines, rasterize_lines_to_strips,
    },
};

fn fixed(value: f32) -> FixedScalar { FixedScalar::from_num(value) }

fn scene() -> Vec<Edge<FixedScalar>> {
    let edge = |x, top, bottom, winding| Edge {
        upper: (fixed(x), fixed(top)).into(),
        lower: (fixed(x), fixed(bottom)).into(), winding,
    };
    vec![
        edge(0.0, 0.0, 16.0, 1), edge(16.0, 0.0, 16.0, -1),
        edge(32.5, 0.5, 17.5, 1), edge(47.5, 0.5, 17.5, -1),
    ]
}

type RasterStorage =
    (Vec<FixedSegment>, Vec<FixedTrapezoid>, Vec<u64>, Vec<u32>, Vec<u32>);

fn raster_workspaces(lines: &[FixedLine], width: u32, height: u32) ->
    RasterStorage {
    let requirements = fixed_strip_requirements(lines, height).unwrap();
    (
        vec![FixedSegment::default(); lines.len()],
        vec![FixedTrapezoid::default(); lines.len().div_ceil(2)],
        vec![0; width as usize],
        vec![0; requirements.offsets],
        vec![0; requirements.indices],
    )
}

#[test] fn tiles_are_compact_sparse_classified_and_exact() {
    assert_eq!(core::mem::size_of::<FixedCoverageTile>(), 16);
    assert_eq!(core::mem::size_of::<FixedCoverageTileRun>(), 4);
    assert_eq!(core::mem::size_of::<FixedCoverageTilePiece>(), 8);

    let edges = scene();
    let mut lines = vec![FixedLine::default(); edges.len()];
    prepare_lines(&edges, &mut lines).unwrap();
    let (width, height) = (48, 18);
    let (mut segments, mut trapezoids, mut row_area, mut offsets, mut indices) =
        raster_workspaces(&lines, width, height);
    let mut raster_workspace = FixedRasterWorkspace {
        segments: &mut segments, trapezoids: &mut trapezoids, row_area: &mut row_area,
        strip_offsets: &mut offsets, strip_indices: &mut indices,
    };
    let (mut strips, mut strip_runs) =
        (vec![FixedCoverageStrip::default(); 4], vec![FixedCoverageRun::default(); 128]);
    let coverage = rasterize_lines_to_strips(&lines, width, height, FillRule::NonZero,
        &mut raster_workspace,
        FixedCoverageWorkspace { strips: &mut strips, runs: &mut strip_runs }).unwrap();
    let (mut tiles, mut tile_runs, mut pieces) = (
        vec![FixedCoverageTile::default(); 8],
        vec![FixedCoverageTileRun::default(); 128],
        vec![FixedCoverageTilePiece::default(); 128],
    );
    let tiled = encode_fixed_coverage_tiles(coverage, FixedCoverageTileWorkspace {
        tiles: &mut tiles, runs: &mut tile_runs, pieces: &mut pieces,
    }).unwrap();

    assert_eq!(tiled.tiles().iter().map(|tile| (tile.x, tile.y, tile.kind))
        .collect::<Vec<_>>(), [
        (0, 0, FixedTileKind::Full),
        (32, 0, FixedTileKind::Boundary),
        (32, 16, FixedTileKind::Boundary),
    ]);
    assert_eq!(tiled.tiles()[0].run_count, 0);
    assert!(!tiled.runs().is_empty());

    let mut direct = vec![0; width as usize * height as usize];
    rasterize_lines(&lines, width, height, FillRule::NonZero, &mut raster_workspace,
        &mut |x, y, coverage| {
            direct[y as usize * width as usize + x as usize] = coverage;
            Ok::<_, Infallible>(())
        }).unwrap();
    let mut replayed = vec![0; direct.len()];
    tiled.replay(&mut |x, y, coverage| {
        replayed[y as usize * width as usize + x as usize] = coverage;
        Ok::<_, Infallible>(())
    }).unwrap();
    assert_eq!(replayed, direct);
}

#[test] fn tile_encoding_reports_each_capacity_without_touching_output() {
    let edges = scene();
    let mut lines = vec![FixedLine::default(); edges.len()];
    prepare_lines(&edges, &mut lines).unwrap();
    let (width, height) = (48, 18);
    let (mut segments, mut trapezoids, mut row_area, mut offsets, mut indices) =
        raster_workspaces(&lines, width, height);
    let mut raster_workspace = FixedRasterWorkspace {
        segments: &mut segments, trapezoids: &mut trapezoids, row_area: &mut row_area,
        strip_offsets: &mut offsets, strip_indices: &mut indices,
    };
    let (mut strips, mut strip_runs) =
        (vec![FixedCoverageStrip::default(); 4], vec![FixedCoverageRun::default(); 128]);
    let coverage = rasterize_lines_to_strips(&lines, width, height, FillRule::NonZero,
        &mut raster_workspace,
        FixedCoverageWorkspace { strips: &mut strips, runs: &mut strip_runs }).unwrap();

    let sentinel_tile = FixedCoverageTile {
        x: 7, y: 9, run_start: 11, run_count: 13, kind: FixedTileKind::Full,
    };
    let sentinel_run = FixedCoverageTileRun { x: 1, len: 2, row: 3, coverage: 4 };
    let (mut tiles, mut runs, mut pieces) = (
        [sentinel_tile; 8], [sentinel_run; 128], [FixedCoverageTilePiece::default(); 128],
    );
    assert_eq!(encode_fixed_coverage_tiles(coverage, FixedCoverageTileWorkspace {
        tiles: &mut tiles, runs: &mut runs, pieces: &mut [],
    }).unwrap_err(), FixedRasterError::WorkspaceTooSmall {
        kind: FixedWorkspace::CoverageTilePieces, required: 1,
    });
    assert_eq!((tiles, runs), ([sentinel_tile; 8], [sentinel_run; 128]));

    assert_eq!(encode_fixed_coverage_tiles(coverage, FixedCoverageTileWorkspace {
        tiles: &mut [], runs: &mut runs, pieces: &mut pieces,
    }).unwrap_err(), FixedRasterError::WorkspaceTooSmall {
        kind: FixedWorkspace::CoverageTiles, required: 3,
    });
    assert_eq!(runs, [sentinel_run; 128]);

    assert!(matches!(encode_fixed_coverage_tiles(coverage, FixedCoverageTileWorkspace {
        tiles: &mut tiles, runs: &mut [], pieces: &mut pieces,
    }), Err(FixedRasterError::WorkspaceTooSmall {
        kind: FixedWorkspace::CoverageTileRuns, required: 1..,
    })));
    assert_eq!(tiles, [sentinel_tile; 8]);
}
