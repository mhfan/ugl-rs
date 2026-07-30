//! Compact tile indexing for retained fixed-point coverage.

use crate::{raster::{CoverageSink, FillRule}, raster_fixed::{
    FixedCoverageStrips, FixedLine, FixedRasterError, FixedRasterWorkspace,
    FixedRenderError, FixedWorkspace, rasterize_lines,
}};

pub const FIXED_TILE_WIDTH:  u32 = 16;
pub const FIXED_TILE_HEIGHT: u32 = 16;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)] pub enum FixedTileKind { Full, #[default] Boundary }

/// One non-empty coverage tile. Empty tiles are omitted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)] pub struct FixedCoverageTile {
    pub x: u32, pub y: u32,
    pub run_start: u32,
    pub run_count: u16,
    pub kind: FixedTileKind,
}

/// A boundary run using tile-local coordinates.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] #[repr(C)]
pub struct FixedCoverageTileRun { pub x: u8, pub len: u8, pub row: u8, pub coverage: u8 }

/// Eight-byte sortable scratch record used while grouping runs by tile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] #[repr(C)]
pub struct FixedCoverageTilePiece { tile: u32, run: FixedCoverageTileRun }

pub struct FixedCoverageTileWorkspace<'a> {
    pub tiles: &'a mut [FixedCoverageTile],
    pub runs: &'a mut [FixedCoverageTileRun],
    pub pieces: &'a mut [FixedCoverageTilePiece],
}

/// Eight-byte linked scratch record used by direct tile-major rasterization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] #[repr(C)]
pub struct FixedDirectTilePiece { run: FixedCoverageTileRun, next: u32 }

/// Caller-owned output and one-strip scratch for direct tile-major rasterization.
pub struct FixedDirectTileWorkspace<'a> {
    pub  tiles: &'a mut [FixedCoverageTile],
    pub   runs: &'a mut [FixedCoverageTileRun],
    pub pieces: &'a mut [FixedDirectTilePiece],
    pub column_heads: &'a mut [u32],
    pub column_tails: &'a mut [u32],
    pub touched_columns: &'a mut [u32],
}

#[derive(Clone, Copy, Debug)] pub struct FixedCoverageTiles<'a> {
    width: u32, height: u32,
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
            let  width = (self.width  - tile.x).min(FIXED_TILE_WIDTH);
            let height = (self.height - tile.y).min(FIXED_TILE_HEIGHT);
            match tile.kind {
                FixedTileKind::Full => {
                    for row in 0..height { sink.span(tile.x, tile.y + row, width, u8::MAX)?; }
                }
                FixedTileKind::Boundary => {
                    let start = tile.run_start as usize;
                    for run in &self.runs[start..start + tile.run_count as usize] {
                        sink.span(tile.x + run.x as u32,
                                  tile.y + run.row as u32, run.len as _, run.coverage)?;
                    }
                }
            }
        }   Ok(())
    }
}

/// Rasterizes directly into sparse tile-major coverage.
///
/// Fine runs are linked per active tile column for one 16-row strip, then
/// compacted into final tile order. Scratch therefore scales with the widest
/// active strip rather than the retained coverage of the whole frame.
pub fn rasterize_lines_to_tiles<'a>(lines: &[FixedLine], width: u32, height: u32,
    fill_rule: FillRule, raster_workspace: &mut FixedRasterWorkspace<'_>,
    workspace: FixedDirectTileWorkspace<'a>) ->
    Result<FixedCoverageTiles<'a>, FixedRasterError> {
    let columns = width.div_ceil(FIXED_TILE_WIDTH) as usize;
    if (columns as u64) * height.div_ceil(FIXED_TILE_HEIGHT) as u64 > u32::MAX as u64 {
        return Err(FixedRasterError::DimensionsOverflow);
    }
    for (available, required) in [
        (workspace.column_heads.len(), columns),
        (workspace.column_tails.len(), columns),
        (workspace.touched_columns.len(), columns)] {
        if available < required {
            return Err(FixedRasterError::WorkspaceTooSmall {
                kind: FixedWorkspace::CoverageTileColumns, required,
            });
        }
    }
    let mut encoder = FixedDirectTileEncoder { width, height, columns,
        tiles: workspace.tiles, runs: workspace.runs, pieces: workspace.pieces,
        heads: &mut workspace.column_heads[..columns],
        tails: &mut workspace.column_tails[..columns],
        touched: &mut workspace.touched_columns[..columns],
        current_strip: None, tile_count: 0, run_count: 0,
        piece_count: 0, touched_count: 0,
    };
    encoder.heads.fill(u32::MAX);
    match rasterize_lines(lines, width, height, fill_rule, raster_workspace, &mut encoder) {
        Ok(()) => encoder.finish(),
        Err(FixedRenderError::Raster(error) | FixedRenderError::Sink(error)) => Err(error),
    }
}

struct FixedDirectTileEncoder<'a> {
    width: u32, height: u32,
    columns: usize,
     tiles: &'a mut [FixedCoverageTile],
      runs: &'a mut [FixedCoverageTileRun],
    pieces: &'a mut [FixedDirectTilePiece],
    heads: &'a mut [u32],
    tails: &'a mut [u32],
    touched: &'a mut [u32],
    current_strip: Option<u32>,
    tile_count: usize,
     run_count: usize,
      piece_count: usize,
    touched_count: usize,
}

impl<'a> FixedDirectTileEncoder<'a> {
    fn finish(mut self) -> Result<FixedCoverageTiles<'a>, FixedRasterError> {
        self.flush_strip()?;
        Ok(FixedCoverageTiles {
            width: self.width, height: self.height,
            tiles: &self.tiles[..self.tile_count], runs: &self.runs[..self.run_count],
        })
    }

    fn flush_strip(&mut self) -> Result<(), FixedRasterError> {
        let Some(strip_y) = self.current_strip else { return Ok(()); };
        let columns = &mut self.touched[..self.touched_count];
        columns.sort_unstable();
        let mut boundary_runs = 0;
        for &column in columns.iter() {
            let  width = (self.width - column * FIXED_TILE_WIDTH).min(FIXED_TILE_WIDTH);
            let height = (self.height - strip_y).min(FIXED_TILE_HEIGHT);
            let (full, count) =
                linked_tile_stats(self.pieces, self.heads[column as usize], width, height);
            self.tails[column as usize] = if full { u32::MAX } else { count as _ };
            if !full { boundary_runs += count; }
        }
        for (kind, available, required) in [
            (FixedWorkspace::CoverageTiles, self.tiles.len(),
                self.tile_count + self.touched_count),
            (FixedWorkspace::CoverageTileRuns, self.runs.len(),
                self.run_count + boundary_runs)] {
            if available < required {
                return Err(FixedRasterError::WorkspaceTooSmall { kind, required });
            }
        }
        if self.run_count + boundary_runs > u32::MAX as usize {
            return Err(FixedRasterError::DimensionsOverflow);
        }

        for &column in columns.iter() {
            let head = self.heads[column as usize];
            let full = self.tails[column as usize] == u32::MAX;
            let run_start = self.run_count;
            if !full {
                let mut piece = head;
                while piece != u32::MAX {
                    self.runs[self.run_count] = self.pieces[piece as usize].run;
                    self.run_count += 1;
                    piece = self.pieces[piece as usize].next;
                }
            }
            self.tiles[self.tile_count] = FixedCoverageTile {
                x: column * FIXED_TILE_WIDTH, y: strip_y,
                run_start: run_start as _, run_count: (self.run_count - run_start) as _,
                kind: if full { FixedTileKind::Full } else { FixedTileKind::Boundary },
            };
            self.tile_count += 1;
            self.heads[column as usize] = u32::MAX;
        }
        self.current_strip = None;
        self.touched_count = 0;
        self.piece_count = 0;
        Ok(())
    }
}

impl CoverageSink for FixedDirectTileEncoder<'_> {
    type Error = FixedRasterError;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        let strip_y = y / FIXED_TILE_HEIGHT * FIXED_TILE_HEIGHT;
        if self.current_strip.is_some_and(|current| current != strip_y) {
            self.flush_strip()?;
        }
        self.current_strip = Some(strip_y);
        let (mut x, end) = (x, x + len);
        while x < end {
            if self.piece_count == self.pieces.len() {
                return Err(FixedRasterError::WorkspaceTooSmall {
                    kind: FixedWorkspace::CoverageTilePieces, required: self.piece_count + 1,
                });
            }
            if self.piece_count == u32::MAX as usize {
                return Err(FixedRasterError::DimensionsOverflow);
            }
            let (column, local_x) =
                ((x / FIXED_TILE_WIDTH) as usize, x % FIXED_TILE_WIDTH);
            debug_assert!(column < self.columns);
            let run_len = (end - x).min(FIXED_TILE_WIDTH - local_x);
            let piece = self.piece_count as u32;
            self.pieces[self.piece_count] = FixedDirectTilePiece {
                run: FixedCoverageTileRun {
                    x: local_x as _, len: run_len as _,
                    row: (y - strip_y) as _, coverage,
                },  next: u32::MAX,
            };
            if  self.heads[column] == u32::MAX {
                self.heads[column] = piece;
                self.touched[self.touched_count] = column as _;
                self.touched_count += 1;
            } else {
                self.pieces[self.tails[column] as usize].next = piece;
            }
            self.tails[column] = piece;
            self.piece_count += 1;
            x += run_len;
        }   Ok(())
    }
}

fn linked_tile_stats(pieces: &[FixedDirectTilePiece], mut piece: u32,
    width: u32, height: u32) -> (bool, usize) {
    let (mut full, mut count, mut row, mut x) = (true, 0, 0, 0);
    while piece != u32::MAX {
        let current = pieces[piece as usize];
        let run = current.run;
        if  run.row as u32 != row {
            full &= x == width && run.row as u32 == row + 1;
            row = run.row as _;     x = 0;
        }
        full &= run.coverage == u8::MAX && run.x as u32 == x;
        x += run.len as u32;
        piece = current.next;
        count += 1;
    }   (full && x == width && row + 1 == height, count)
}

/// Converts retained row-major strips into sparse tile-major coverage.
///
/// Empty tiles are omitted and full tiles carry no fine runs. Output buffers
/// remain untouched if capacity validation fails.
pub fn encode_fixed_coverage_tiles<'a>(coverage: FixedCoverageStrips<'_>,
    workspace: FixedCoverageTileWorkspace<'a>) ->
    Result<FixedCoverageTiles<'a>, FixedRasterError> {
    let columns = coverage. width().div_ceil(FIXED_TILE_WIDTH);
    let    rows = coverage.height().div_ceil(FIXED_TILE_HEIGHT);
    if (columns as u64) * rows as u64 > u32::MAX as u64 {
        return Err(FixedRasterError::DimensionsOverflow);
    }

    let mut piece_count = 0;
    for strip in coverage.strips() {
        let strip_piece_start = piece_count;
        let start = strip.run_start as usize;
        for run in &coverage.runs()[start..start + strip.run_count as usize] {
            let (mut x, end) = (run.x, run.x + run.len);
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
        workspace.pieces[strip_piece_start..piece_count]
            .sort_unstable_by_key(|piece| (piece.tile, piece.run.row, piece.run.x));
    }
    let pieces = &mut workspace.pieces[..piece_count];

    let (mut tile_count, mut boundary_runs, mut index) = (0, 0, 0);
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
    }   index == pieces.len()
}

#[cfg(test)] mod tests { use super::*;
    use alloc::{vec, vec::Vec};
    use core::convert::Infallible;
    use crate::{edge::Edge, geometry::FixedScalar, raster::FillRule,
        raster_fixed::{FixedCoverageRun, FixedCoverageStrip, FixedCoverageWorkspace,
            FixedLine, FixedRasterWorkspace, FixedSegment, FixedTrapezoid,
            fixed_strip_requirements, prepare_lines, rasterize_lines,
            rasterize_lines_to_strips,
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
            vec![FixedSegment::default();   lines.len()],
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
        assert_eq!(core::mem::size_of::<FixedDirectTilePiece>(), 8);

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

        assert_eq!(tiled.tiles().iter().map(|tile|
            (tile.x, tile.y, tile.kind)).collect::<Vec<_>>(), [
            (0, 0, FixedTileKind::Full),
            (32, 0, FixedTileKind::Boundary),
            (32, 16, FixedTileKind::Boundary),
        ]);
        assert_eq!(tiled.tiles()[0].run_count, 0);
        assert!(!tiled.runs().is_empty());

        let (mut direct_tiles, mut direct_runs, mut direct_pieces) = (
            vec![FixedCoverageTile::default(); 8],
            vec![FixedCoverageTileRun::default(); 128],
            vec![FixedDirectTilePiece::default(); 128],
        );
        let (mut heads, mut tails, mut touched) = ([0; 3], [0; 3], [0; 3]);
        let direct_tiled = rasterize_lines_to_tiles(&lines, width, height,
            FillRule::NonZero, &mut raster_workspace, FixedDirectTileWorkspace {
                tiles: &mut direct_tiles, runs: &mut direct_runs,
                pieces: &mut direct_pieces, column_heads: &mut heads,
                column_tails: &mut tails, touched_columns: &mut touched,
            }).unwrap();
        assert_eq!(direct_tiled.tiles(), tiled.tiles());
        assert_eq!(direct_tiled.runs(), tiled.runs());

        let mut streamed = vec![0; width as usize * height as usize];
        rasterize_lines(&lines, width, height, FillRule::NonZero,
            &mut raster_workspace, &mut |x, y, coverage| {
                streamed[y as usize * width as usize + x as usize] = coverage;
                Ok::<_, Infallible>(())
            }).unwrap();
        let mut replayed = vec![0; streamed.len()];
        direct_tiled.replay(&mut |x, y, coverage| {
            replayed[y as usize * width as usize + x as usize] = coverage;
            Ok::<_, Infallible>(())
        }).unwrap();
        assert_eq!(replayed, streamed);
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
        let mut strips     = vec![FixedCoverageStrip::default(); 4];
        let mut strip_runs = vec![FixedCoverageRun::default(); 128];
        let coverage = rasterize_lines_to_strips(&lines, width, height,
            FillRule::NonZero, &mut raster_workspace, FixedCoverageWorkspace {
                strips: &mut strips, runs: &mut strip_runs }).unwrap();

        let sentinel_tile = FixedCoverageTile {
            x: 7, y: 9, run_start: 11, run_count: 13, kind: FixedTileKind::Full,
        };
        let sentinel_run = FixedCoverageTileRun { x: 1, len: 2, row: 3, coverage: 4 };
        let (mut tiles, mut runs) = ([sentinel_tile; 8], [sentinel_run; 128]);
        let mut pieces = [FixedCoverageTilePiece::default(); 128];
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

    #[test] fn direct_tile_raster_reports_column_piece_and_output_capacity() {
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

        let mut  tiles = [FixedCoverageTile::default(); 8];
        let mut   runs = [FixedCoverageTileRun::default(); 128];
        let mut pieces = [FixedDirectTilePiece::default(); 128];
        let (mut heads, mut tails, mut touched) = ([0; 3], [0; 3], [0; 3]);

        assert_eq!(rasterize_lines_to_tiles(&lines, width, height, FillRule::NonZero,
            &mut raster_workspace, FixedDirectTileWorkspace {
                tiles: &mut tiles, runs: &mut runs, pieces: &mut pieces,
                column_heads: &mut heads[..2], column_tails: &mut tails,
                touched_columns: &mut touched,
            }).unwrap_err(), FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::CoverageTileColumns, required: 3,
        });
        assert_eq!(rasterize_lines_to_tiles(&lines, width, height, FillRule::NonZero,
            &mut raster_workspace, FixedDirectTileWorkspace {
                tiles: &mut tiles, runs: &mut runs, pieces: &mut [],
                column_heads: &mut heads, column_tails: &mut tails,
                touched_columns: &mut touched,
            }).unwrap_err(), FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::CoverageTilePieces, required: 1,
        });
        assert_eq!(rasterize_lines_to_tiles(&lines, width, height, FillRule::NonZero,
            &mut raster_workspace, FixedDirectTileWorkspace {
                tiles: &mut [], runs: &mut runs, pieces: &mut pieces,
                column_heads: &mut heads, column_tails: &mut tails,
                touched_columns: &mut touched,
            }).unwrap_err(), FixedRasterError::WorkspaceTooSmall {
            kind: FixedWorkspace::CoverageTiles, required: 2,
        });
        assert!(matches!(rasterize_lines_to_tiles(&lines, width, height,
            FillRule::NonZero, &mut raster_workspace, FixedDirectTileWorkspace {
                tiles: &mut tiles, runs: &mut [], pieces: &mut pieces,
                column_heads: &mut heads, column_tails: &mut tails,
                touched_columns: &mut touched,
            }), Err(FixedRasterError::WorkspaceTooSmall {
                kind: FixedWorkspace::CoverageTileRuns, required: 1..,
            })));
    }
}
