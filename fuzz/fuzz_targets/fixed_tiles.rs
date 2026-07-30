#![no_main]

use core::convert::Infallible;
use libfuzzer_sys::fuzz_target;
use ugl_rs::{edge::Edge, geometry::{FixedScalar, Point}, raster::FillRule,
    raster_fixed::{
        FIXED_STRIP_HEIGHT, FixedCoverageRun, FixedCoverageStrip, FixedCoverageWorkspace,
        FixedLine, FixedRasterWorkspace, FixedRenderError, FixedSegment, FixedTrapezoid,
        fixed_strip_requirements, prepare_lines, rasterize_lines, rasterize_lines_to_strips,
    },
    tile_fixed::{
        FixedCoverageTile, FixedCoverageTileRun, FixedDirectTilePiece,
        FixedDirectTileWorkspace, fixed_tile_requirements, rasterize_lines_to_tiles,
    },
};

fn point(bytes: [u8; 4], width: u32, height: u32) -> Point<FixedScalar> {
    let coordinate = |bytes: [u8; 2], extent: u32| {
        let range = (extent + 8) * 256;
        FixedScalar::from_bits(u16::from_le_bytes(bytes) as i32 % range as i32 - 1024)
    };
    (coordinate([bytes[0], bytes[1]], width),
     coordinate([bytes[2], bytes[3]], height)).into()
}

fn edge(from: Point<FixedScalar>, to: Point<FixedScalar>) -> Option<Edge<FixedScalar>> {
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

    let mut lines = vec![FixedLine::default(); edges.len()];
    let line_count = prepare_lines(&edges, &mut lines).unwrap();
    lines.truncate(line_count);
    let strip_requirements = fixed_strip_requirements(&lines, height).unwrap();
    let tile_requirements = fixed_tile_requirements(width, height).unwrap();
    let (mut segments, mut trapezoids, mut row_area) = (
        vec![FixedSegment::default(); lines.len()],
        vec![FixedTrapezoid::default(); lines.len().div_ceil(2)],
        vec![0; width as usize],
    );
    let (mut offsets, mut indices) = (
        vec![0; strip_requirements.offsets], vec![0; strip_requirements.indices],
    );
    let (mut strips, mut strip_runs) = (
        vec![FixedCoverageStrip::default(); height.div_ceil(FIXED_STRIP_HEIGHT) as usize],
        vec![FixedCoverageRun::default(); width as usize * height as usize],
    );
    let (mut tiles, mut tile_runs, mut pieces) = (
        vec![FixedCoverageTile::default(); tile_requirements.tiles],
        vec![FixedCoverageTileRun::default(); tile_requirements.runs],
        vec![FixedDirectTilePiece::default(); tile_requirements.pieces],
    );
    let (mut heads, mut tails, mut touched) = (
        vec![0; tile_requirements.columns], vec![0; tile_requirements.columns],
        vec![0; tile_requirements.columns],
    );

    for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
        let mut workspace = FixedRasterWorkspace {
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
            let FixedRenderError::Raster(error) = error;
            assert_eq!(rasterize_lines_to_strips(&lines, width, height, fill_rule,
                &mut workspace, FixedCoverageWorkspace {
                    strips: &mut strips, runs: &mut strip_runs,
                }).unwrap_err(), error);
            assert_eq!(rasterize_lines_to_tiles(&lines, width, height, fill_rule,
                &mut workspace, FixedDirectTileWorkspace {
                    tiles: &mut tiles, runs: &mut tile_runs, pieces: &mut pieces,
                    column_heads: &mut heads, column_tails: &mut tails,
                    touched_columns: &mut touched,
                }).unwrap_err(), error);
            continue;
        }

        let retained = rasterize_lines_to_strips(&lines, width, height, fill_rule,
            &mut workspace, FixedCoverageWorkspace {
                strips: &mut strips, runs: &mut strip_runs,
            }).unwrap();
        let mut replayed = vec![0; streamed.len()];
        retained.replay(&mut |x, y, coverage| {
            replayed[y as usize * width as usize + x as usize] = coverage;
            Ok::<_, Infallible>(())
        }).unwrap();
        assert_eq!(replayed, streamed);

        let tiled = rasterize_lines_to_tiles(&lines, width, height, fill_rule,
            &mut workspace, FixedDirectTileWorkspace {
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
