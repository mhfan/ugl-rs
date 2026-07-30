//! Compact tile indexing for retained fixed-point coverage.

use crate::{raster::CoverageSink, raster_fixed::{
    FixedCoverageStrips, FixedRasterError, FixedWorkspace,
}};

pub const FIXED_TILE_WIDTH: u32 = 16;
pub const FIXED_TILE_HEIGHT: u32 = 16;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FixedTileKind { Full, #[default] Boundary }

/// One non-empty coverage tile. Empty tiles are omitted.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedCoverageTile {
    pub x: u32,
    pub y: u32,
    pub run_start: u32,
    pub run_count: u16,
    pub kind: FixedTileKind,
}

/// A boundary run using tile-local coordinates.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedCoverageTileRun { pub x: u8, pub len: u8, pub row: u8, pub coverage: u8 }

/// Eight-byte sortable scratch record used while grouping runs by tile.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedCoverageTilePiece { tile: u32, run: FixedCoverageTileRun }

pub struct FixedCoverageTileWorkspace<'a> {
    pub tiles: &'a mut [FixedCoverageTile],
    pub runs: &'a mut [FixedCoverageTileRun],
    pub pieces: &'a mut [FixedCoverageTilePiece],
}

#[derive(Clone, Copy, Debug)]
pub struct FixedCoverageTiles<'a> {
    width: u32,
    height: u32,
    tiles: &'a [FixedCoverageTile],
    runs: &'a [FixedCoverageTileRun],
}

impl<'a> FixedCoverageTiles<'a> {
    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn tiles(&self) -> &'a [FixedCoverageTile] { self.tiles }
    pub fn runs(&self) -> &'a [FixedCoverageTileRun] { self.runs }

    pub fn replay<S: CoverageSink>(&self, sink: &mut S) -> Result<(), S::Error> {
        for tile in self.tiles {
            let (width, height) = (
                (self.width - tile.x).min(FIXED_TILE_WIDTH),
                (self.height - tile.y).min(FIXED_TILE_HEIGHT),
            );
            match tile.kind {
                FixedTileKind::Full => {
                    for row in 0..height { sink.span(tile.x, tile.y + row, width, u8::MAX)?; }
                }
                FixedTileKind::Boundary => {
                    let start = tile.run_start as usize;
                    for run in &self.runs[start..start + tile.run_count as usize] {
                        sink.span(tile.x + run.x as u32, tile.y + run.row as u32,
                            run.len as _, run.coverage)?;
                    }
                }
            }
        }   Ok(())
    }
}

/// Converts retained row-major strips into sparse tile-major coverage.
///
/// Empty tiles are omitted and full tiles carry no fine runs. Output buffers
/// remain untouched if capacity validation fails.
pub fn encode_fixed_coverage_tiles<'a>(coverage: FixedCoverageStrips<'_>,
    workspace: FixedCoverageTileWorkspace<'a>) ->
    Result<FixedCoverageTiles<'a>, FixedRasterError> {
    let columns = coverage.width().div_ceil(FIXED_TILE_WIDTH);
    let rows = coverage.height().div_ceil(FIXED_TILE_HEIGHT);
    if (columns as u64) * rows as u64 > u32::MAX as u64 {
        return Err(FixedRasterError::DimensionsOverflow);
    }

    let mut piece_count = 0;
    for strip in coverage.strips() {
        let start = strip.run_start as usize;
        for run in &coverage.runs()[start..start + strip.run_count as usize] {
            let mut x = run.x;
            let end = x + run.len;
            while x < end {
                if piece_count == workspace.pieces.len() {
                    return Err(FixedRasterError::WorkspaceTooSmall {
                        kind: FixedWorkspace::CoverageTilePieces, required: piece_count + 1,
                    });
                }
                let (tile_x, local_x) = (x / FIXED_TILE_WIDTH, x % FIXED_TILE_WIDTH);
                let len = (end - x).min(FIXED_TILE_WIDTH - local_x);
                workspace.pieces[piece_count] = FixedCoverageTilePiece {
                    tile: (strip.y / FIXED_TILE_HEIGHT) * columns + tile_x,
                    run: FixedCoverageTileRun {
                        x: local_x as _, len: len as _, row: run.row, coverage: run.coverage,
                    },
                };
                piece_count += 1;
                x += len;
            }
        }
    }
    let pieces = &mut workspace.pieces[..piece_count];
    pieces.sort_unstable_by_key(|piece| (piece.tile, piece.run.row, piece.run.x));

    let mut tile_count = 0;
    let mut boundary_runs = 0;
    let mut index = 0;
    while index < pieces.len() {
        let end = tile_group_end(pieces, index);
        let tile = pieces[index].tile;
        let (tile_x, tile_y) = (tile % columns, tile / columns);
        let full = tile_is_full(&pieces[index..end],
            (coverage.width() - tile_x * FIXED_TILE_WIDTH).min(FIXED_TILE_WIDTH),
            (coverage.height() - tile_y * FIXED_TILE_HEIGHT).min(FIXED_TILE_HEIGHT));
        tile_count += 1;
        if !full { boundary_runs += end - index; }
        index = end;
    }
    for (kind, available, required) in [
        (FixedWorkspace::CoverageTiles, workspace.tiles.len(), tile_count),
        (FixedWorkspace::CoverageTileRuns, workspace.runs.len(), boundary_runs)] {
        if available < required {
            return Err(FixedRasterError::WorkspaceTooSmall { kind, required });
        }
    }
    if boundary_runs > u32::MAX as usize {
        return Err(FixedRasterError::DimensionsOverflow);
    }

    let (mut tile_count, mut run_count, mut index) = (0, 0, 0);
    while index < pieces.len() {
        let end = tile_group_end(pieces, index);
        let tile = pieces[index].tile;
        let (tile_x, tile_y) = (tile % columns, tile / columns);
        let (width, height) = (
            (coverage.width() - tile_x * FIXED_TILE_WIDTH).min(FIXED_TILE_WIDTH),
            (coverage.height() - tile_y * FIXED_TILE_HEIGHT).min(FIXED_TILE_HEIGHT),
        );
        let full = tile_is_full(&pieces[index..end], width, height);
        let count = if full { 0 } else { end - index };
        workspace.tiles[tile_count] = FixedCoverageTile {
            x: tile_x * FIXED_TILE_WIDTH, y: tile_y * FIXED_TILE_HEIGHT,
            run_start: run_count as _, run_count: count as _,
            kind: if full { FixedTileKind::Full } else { FixedTileKind::Boundary },
        };
        if !full {
            for piece in &pieces[index..end] {
                workspace.runs[run_count] = piece.run;
                run_count += 1;
            }
        }
        tile_count += 1;
        index = end;
    }
    Ok(FixedCoverageTiles {
        width: coverage.width(), height: coverage.height(),
        tiles: &workspace.tiles[..tile_count], runs: &workspace.runs[..run_count],
    })
}

fn tile_group_end(pieces: &[FixedCoverageTilePiece], start: usize) -> usize {
    let tile = pieces[start].tile;
    start + pieces[start..].partition_point(|piece| piece.tile == tile)
}

fn tile_is_full(pieces: &[FixedCoverageTilePiece], width: u32, height: u32) -> bool {
    let (mut index, mut row) = (0, 0);
    while row < height {
        let mut x = 0;
        while index < pieces.len() && pieces[index].run.row as u32 == row {
            let run = pieces[index].run;
            if run.coverage != u8::MAX || run.x as u32 != x { return false; }
            x += run.len as u32;
            index += 1;
        }
        if x != width { return false; }
        row += 1;
    }
    index == pieces.len()
}

#[cfg(test)] #[path = "tile_tests.rs"] mod tests;
