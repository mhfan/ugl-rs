//! Compact tile indexing for retained fixed-point coverage.

use crate::{fixed::{DEVICE_RAW_LIMIT, raster::{
        CoverageStrips, Error, Line, RenderError, SUBPIXEL_SCALE, Workspace,
        WorkspaceKind, rasterize_lines,
    }}, raster::{CoverageSink, FillRule}};

pub const TILE_WIDTH:  u32 = 16;
pub const TILE_HEIGHT: u32 = 16;

/// Conservative capacities for allocation-free direct tile rasterization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Requirements {
    pub columns: usize,
    pub  pieces: usize,
    pub tiles: usize,
     pub runs: usize,
}

/// Computes worst-case direct-tile capacities for a target.
///
/// `columns` elements are required in each column scratch slice. `tiles`,
/// `runs`, and `pieces` bound their correspondingly named workspace slices.
pub fn requirements(width: u32, height: u32) ->
    Result<Requirements, Error> {
    let extent = |value: u32| value as u64 * SUBPIXEL_SCALE as u64;
    if extent(width) > DEVICE_RAW_LIMIT as u64 || extent(height) > DEVICE_RAW_LIMIT as u64 {
        return Err(Error::CoordinateOutOfRange);
    }
    let  width = usize::try_from(width) .map_err(|_| Error::DimensionsOverflow)?;
    let height = usize::try_from(height).map_err(|_| Error::DimensionsOverflow)?;
    let tile_width = TILE_WIDTH as usize;
    let tile_height = TILE_HEIGHT as usize;
    let columns = width.div_ceil(tile_width);
    let rows = height.div_ceil(tile_height);
    let checked_mul = |left: usize, right| left.checked_mul(right)
        .ok_or(Error::DimensionsOverflow);
    let tiles = checked_mul(columns, rows)?;
    if  tiles > u32::MAX as usize { return Err(Error::DimensionsOverflow); }
    Ok(Requirements { columns, tiles,
          runs: checked_mul(width, height)?,
        pieces: checked_mul(width, height.min(tile_height))?,
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)] pub enum TileKind { Full, #[default] Boundary }

/// One non-empty coverage tile. Empty tiles are omitted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)] pub struct CoverageTile {
    pub x: u32, pub y: u32,
    pub run_start: u32,
    pub run_count: u16,
    pub kind: TileKind,
}

/// A boundary run using tile-local coordinates.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] #[repr(C)]
pub struct CoverageTileRun { pub x: u8, pub len: u8, pub row: u8, pub coverage: u8 }

/// Eight-byte sortable scratch record used while grouping runs by tile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] #[repr(C)]
pub struct CoverageTilePiece { tile: u32, run: CoverageTileRun }

pub struct CoverageTileWorkspace<'output, 'scratch> {
    pub tiles: &'output mut [CoverageTile],
     pub runs: &'output mut [CoverageTileRun],
    pub pieces: &'scratch mut [CoverageTilePiece],
}

/// Eight-byte linked scratch record used by direct tile-major rasterization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)] #[repr(C)]
pub struct DirectTilePiece { run: CoverageTileRun, next: u32 }

/// Caller-owned output and one-strip scratch for direct tile-major rasterization.
pub struct DirectTileWorkspace<'output, 'scratch> {
    pub tiles: &'output mut [CoverageTile],
    pub  runs: &'output mut [CoverageTileRun],
    pub pieces: &'scratch mut [DirectTilePiece],
    pub column_heads: &'scratch mut [u32],
    pub column_tails: &'scratch mut [u32],
    pub touched_columns: &'scratch mut [u32],
}

#[derive(Clone, Copy, Debug)] pub struct CoverageTiles<'a> {
    width: u32, height: u32,
    tiles: &'a [CoverageTile],
     runs: &'a [CoverageTileRun],
}

impl<'a> CoverageTiles<'a> {
    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn tiles(&self) -> &'a [CoverageTile] { self.tiles }
    pub fn runs(&self) -> &'a [CoverageTileRun] { self.runs }

    pub(crate) fn tile_extent(&self, tile: CoverageTile) -> (u32, u32) {
        ((self.width  - tile.x).min(TILE_WIDTH),
         (self.height - tile.y).min(TILE_HEIGHT))
    }

    pub fn replay<S: CoverageSink>(&self, sink: &mut S) -> Result<(), S::Error> {
        for tile in self.tiles {
            let (width, height) = self.tile_extent(*tile);
            match tile.kind {
                TileKind::Full => {
                    for row in 0..height { sink.span(tile.x, tile.y + row, width, u8::MAX)?; }
                }
                TileKind::Boundary => {
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
/// active strip rather than the retained coverage of the whole frame. On
/// error, callers must discard the formal tile and run outputs; earlier strips
/// may already have been written.
pub fn rasterize_lines_to_tiles<'a>(lines: &[Line], width: u32, height: u32,
    fill_rule: FillRule, raster_workspace: &mut Workspace<'_>,
    workspace: DirectTileWorkspace<'a, '_>) ->
    Result<CoverageTiles<'a>, Error> {
    let requirements = requirements(width, height)?;
    let columns = requirements.columns;
    for (available, required) in [
        (workspace.column_heads.len(), columns),
        (workspace.column_tails.len(), columns),
        (workspace.touched_columns.len(), columns)] {
        if available < required {
            return Err(Error::WorkspaceTooSmall {
                kind: WorkspaceKind::CoverageTileColumns, required,
            });
        }
    }
    let mut encoder = DirectTileEncoder { width, height, columns,
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
        Err(RenderError::Raster(error) | RenderError::Sink(error)) => Err(error),
    }
}

struct DirectTileEncoder<'output, 'scratch> {
    width: u32, height: u32,
    columns: usize,
    tiles: &'output mut [CoverageTile],
     runs: &'output mut [CoverageTileRun],
    pieces: &'scratch mut [DirectTilePiece],
     heads: &'scratch mut [u32],
     tails: &'scratch mut [u32],
    touched: &'scratch mut [u32],
    current_strip: Option<u32>,
    tile_count: usize,
     run_count: usize,
      piece_count: usize,
    touched_count: usize,
}

impl<'output> DirectTileEncoder<'output, '_> {
    fn finish(mut self) -> Result<CoverageTiles<'output>, Error> {
        self.flush_strip()?;
        Ok(CoverageTiles {
            width: self.width, height: self.height,
            tiles: &self.tiles[..self.tile_count], runs: &self.runs[..self.run_count],
        })
    }

    fn flush_strip(&mut self) -> Result<(), Error> {
        let Some(strip_y) = self.current_strip else { return Ok(()); };
        let columns = &mut self.touched[..self.touched_count];
        columns.sort_unstable();
        let mut boundary_runs = 0;
        for &column in columns.iter() {
            let  width = (self.width - column * TILE_WIDTH).min(TILE_WIDTH);
            let height = (self.height - strip_y).min(TILE_HEIGHT);
            let (full, count) =
                linked_tile_stats(self.pieces, self.heads[column as usize], width, height);
            self.tails[column as usize] = if full { u32::MAX } else { count as _ };
            if !full { boundary_runs += count; }
        }
        for (kind, available, required) in [
            (WorkspaceKind::CoverageTiles, self.tiles.len(),
                self.tile_count + self.touched_count),
            (WorkspaceKind::CoverageTileRuns, self.runs.len(),
                self.run_count + boundary_runs)] {
            if available < required {
                return Err(Error::WorkspaceTooSmall { kind, required });
            }
        }
        if self.run_count + boundary_runs > u32::MAX as usize {
            return Err(Error::DimensionsOverflow);
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
            self.tiles[self.tile_count] = CoverageTile {
                x: column * TILE_WIDTH, y: strip_y,
                run_start: run_start as _, run_count: (self.run_count - run_start) as _,
                kind: if full { TileKind::Full } else { TileKind::Boundary },
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

impl CoverageSink for DirectTileEncoder<'_, '_> {
    type Error = Error;

    fn span(&mut self, x: u32, y: u32, len: u32, coverage: u8) ->
        Result<(), Self::Error> {
        let strip_y = y / TILE_HEIGHT * TILE_HEIGHT;
        if self.current_strip.is_some_and(|current| current != strip_y) {
            self.flush_strip()?;
        }
        self.current_strip = Some(strip_y);
        let (mut x, end) = (x, x + len);
        while x < end {
            if self.piece_count == self.pieces.len() {
                return Err(Error::WorkspaceTooSmall {
                    kind: WorkspaceKind::CoverageTilePieces, required: self.piece_count + 1,
                });
            }
            if self.piece_count == u32::MAX as usize {
                return Err(Error::DimensionsOverflow);
            }
            let (column, local_x) =
                ((x / TILE_WIDTH) as usize, x % TILE_WIDTH);
            debug_assert!(column < self.columns);
            let run_len = (end - x).min(TILE_WIDTH - local_x);
            let piece = self.piece_count as u32;
            self.pieces[self.piece_count] = DirectTilePiece {
                run: CoverageTileRun {
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

fn linked_tile_stats(pieces: &[DirectTilePiece], mut piece: u32,
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
pub fn encode_coverage_tiles<'a>(coverage: CoverageStrips<'_>,
    workspace: CoverageTileWorkspace<'a, '_>) ->
    Result<CoverageTiles<'a>, Error> {
    let columns = coverage. width().div_ceil(TILE_WIDTH);
    let    rows = coverage.height().div_ceil(TILE_HEIGHT);
    if (columns as u64) * rows as u64 > u32::MAX as u64 {
        return Err(Error::DimensionsOverflow);
    }

    let mut piece_count = 0;
    for strip in coverage.strips() {
        let strip_piece_start = piece_count;
        let start = strip.run_start as usize;
        for run in &coverage.runs()[start..start + strip.run_count as usize] {
            let (mut x, end) = (run.x, run.x + run.len);
            while x < end {
                if piece_count == workspace.pieces.len() {
                    return Err(Error::WorkspaceTooSmall {
                        kind: WorkspaceKind::CoverageTilePieces, required: piece_count + 1,
                    });
                }
                let (tile_x, local_x) = (x / TILE_WIDTH, x % TILE_WIDTH);
                let len = (end - x).min(TILE_WIDTH - local_x);
                workspace.pieces[piece_count] = CoverageTilePiece {
                    tile: (strip.y / TILE_HEIGHT) * columns + tile_x,
                    run: CoverageTileRun {
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
            (coverage.width() - tile_x * TILE_WIDTH).min(TILE_WIDTH),
            (coverage.height() - tile_y * TILE_HEIGHT).min(TILE_HEIGHT));
        tile_count += 1;
        if !full { boundary_runs += end - index; }
        index = end;
    }
    for (kind, available, required) in [
        (WorkspaceKind::CoverageTiles, workspace.tiles.len(), tile_count),
        (WorkspaceKind::CoverageTileRuns, workspace.runs.len(), boundary_runs)] {
        if available < required {
            return Err(Error::WorkspaceTooSmall { kind, required });
        }
    }
    if boundary_runs > u32::MAX as usize {
        return Err(Error::DimensionsOverflow);
    }

    let (mut tile_count, mut run_count, mut index) = (0, 0, 0);
    while index < pieces.len() {
        let end = tile_group_end(pieces, index);
        let tile = pieces[index].tile;
        let (tile_x, tile_y) = (tile % columns, tile / columns);
        let (width, height) = (
            (coverage.width() - tile_x * TILE_WIDTH).min(TILE_WIDTH),
            (coverage.height() - tile_y * TILE_HEIGHT).min(TILE_HEIGHT),
        );
        let full = tile_is_full(&pieces[index..end], width, height);
        let count = if full { 0 } else { end - index };
        workspace.tiles[tile_count] = CoverageTile {
            x: tile_x * TILE_WIDTH, y: tile_y * TILE_HEIGHT,
            run_start: run_count as _, run_count: count as _,
            kind: if full { TileKind::Full } else { TileKind::Boundary },
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
    Ok(CoverageTiles {
        width: coverage.width(), height: coverage.height(),
        tiles: &workspace.tiles[..tile_count], runs: &workspace.runs[..run_count],
    })
}

fn tile_group_end(pieces: &[CoverageTilePiece], start: usize) -> usize {
    let tile = pieces[start].tile;
    start + pieces[start..].partition_point(|piece| piece.tile == tile)
}

fn tile_is_full(pieces: &[CoverageTilePiece], width: u32, height: u32) -> bool {
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
    use crate::{edge::Edge, fixed::Scalar, raster::FillRule,
        fixed::raster::{CoverageRun, CoverageStrip, CoverageWorkspace,
            Line, Workspace, RenderError, Segment, Trapezoid,
            STRIP_HEIGHT, strip_requirements, prepare_lines, rasterize_lines,
            rasterize_lines_to_strips,
        },
    };

    fn fixed(value: f32) -> Scalar { Scalar::from_num(value) }

    fn scene() -> Vec<Edge<Scalar>> {
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
        (Vec<Segment>, Vec<Trapezoid>, Vec<u64>, Vec<u32>, Vec<u32>);

    fn raster_workspaces(lines: &[Line], width: u32, height: u32) ->
        RasterStorage {
        let requirements = strip_requirements(lines, height).unwrap();
        (
            vec![Segment::default();   lines.len()],
            vec![Trapezoid::default(); lines.len().div_ceil(2)],
            vec![0; width as usize],
            vec![0; requirements.offsets],
            vec![0; requirements.indices],
        )
    }

    #[test] fn tile_requirements_bound_each_workspace_dimension() {
        assert_eq!(requirements(0, 0).unwrap(), Requirements::default());
        assert_eq!(requirements(17, 18).unwrap(),
            Requirements { columns: 2, tiles: 4, runs: 306, pieces: 272 });
        assert_eq!(requirements(DEVICE_RAW_LIMIT as u32 / SUBPIXEL_SCALE + 1, 1),
            Err(Error::CoordinateOutOfRange));
    }

    #[test] fn tiles_are_compact_sparse_classified_and_exact() {
        assert_eq!(core::mem::size_of::<CoverageTile>(), 16);
        assert_eq!(core::mem::size_of::<CoverageTileRun>(), 4);
        assert_eq!(core::mem::size_of::<CoverageTilePiece>(), 8);
        assert_eq!(core::mem::size_of::<DirectTilePiece>(), 8);

        let edges = scene();
        let mut lines = vec![Line::default(); edges.len()];
        prepare_lines(&edges, &mut lines).unwrap();
        let (width, height) = (48, 18);
        let (mut segments, mut trapezoids, mut row_area, mut offsets, mut indices) =
            raster_workspaces(&lines, width, height);
        let mut raster_workspace = Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut offsets, strip_indices: &mut indices,
        };
        let (mut strips, mut strip_runs) =
            (vec![CoverageStrip::default(); 4], vec![CoverageRun::default(); 128]);
        let coverage = rasterize_lines_to_strips(&lines, width, height, FillRule::NonZero,
            &mut raster_workspace,
            CoverageWorkspace { strips: &mut strips, runs: &mut strip_runs }).unwrap();
        let (mut tiles, mut tile_runs, mut pieces) = (
            vec![CoverageTile::default(); 8],
            vec![CoverageTileRun::default(); 128],
            vec![CoverageTilePiece::default(); 128],
        );
        let tiled = encode_coverage_tiles(coverage, CoverageTileWorkspace {
            tiles: &mut tiles, runs: &mut tile_runs, pieces: &mut pieces,
        }).unwrap();

        assert_eq!(tiled.tiles().iter().map(|tile|
            (tile.x, tile.y, tile.kind)).collect::<Vec<_>>(), [
            (0, 0, TileKind::Full),
            (32, 0, TileKind::Boundary),
            (32, 16, TileKind::Boundary),
        ]);
        assert_eq!(tiled.tiles()[0].run_count, 0);
        assert!(!tiled.runs().is_empty());

        let (mut direct_tiles, mut direct_runs, mut direct_pieces) = (
            vec![CoverageTile::default(); 8],
            vec![CoverageTileRun::default(); 128],
            vec![DirectTilePiece::default(); 128],
        );
        let (mut heads, mut tails, mut touched) = ([0; 3], [0; 3], [0; 3]);
        let direct_tiled = rasterize_lines_to_tiles(&lines, width, height,
            FillRule::NonZero, &mut raster_workspace, DirectTileWorkspace {
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
        let mut lines = vec![Line::default(); edges.len()];
        prepare_lines(&edges, &mut lines).unwrap();
        let (width, height) = (48, 18);
        let (mut segments, mut trapezoids, mut row_area, mut offsets, mut indices) =
            raster_workspaces(&lines, width, height);
        let mut raster_workspace = Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut offsets, strip_indices: &mut indices,
        };
        let mut strips     = vec![CoverageStrip::default(); 4];
        let mut strip_runs = vec![CoverageRun::default(); 128];
        let coverage = rasterize_lines_to_strips(&lines, width, height,
            FillRule::NonZero, &mut raster_workspace, CoverageWorkspace {
                strips: &mut strips, runs: &mut strip_runs }).unwrap();

        let sentinel_tile = CoverageTile {
            x: 7, y: 9, run_start: 11, run_count: 13, kind: TileKind::Full,
        };
        let sentinel_run = CoverageTileRun { x: 1, len: 2, row: 3, coverage: 4 };
        let (mut tiles, mut runs) = ([sentinel_tile; 8], [sentinel_run; 128]);
        let mut pieces = [CoverageTilePiece::default(); 128];
        assert_eq!(encode_coverage_tiles(coverage, CoverageTileWorkspace {
            tiles: &mut tiles, runs: &mut runs, pieces: &mut [],
        }).unwrap_err(), Error::WorkspaceTooSmall {
            kind: WorkspaceKind::CoverageTilePieces, required: 1,
        });
        assert_eq!((tiles, runs), ([sentinel_tile; 8], [sentinel_run; 128]));

        assert_eq!(encode_coverage_tiles(coverage, CoverageTileWorkspace {
            tiles: &mut [], runs: &mut runs, pieces: &mut pieces,
        }).unwrap_err(), Error::WorkspaceTooSmall {
            kind: WorkspaceKind::CoverageTiles, required: 3,
        });
        assert_eq!(runs, [sentinel_run; 128]);

        assert!(matches!(encode_coverage_tiles(coverage, CoverageTileWorkspace {
            tiles: &mut tiles, runs: &mut [], pieces: &mut pieces,
        }), Err(Error::WorkspaceTooSmall {
            kind: WorkspaceKind::CoverageTileRuns, required: 1..,
        })));
        assert_eq!(tiles, [sentinel_tile; 8]);
    }

    #[test] fn direct_tile_raster_reports_column_piece_and_output_capacity() {
        let edges = scene();
        let mut lines = vec![Line::default(); edges.len()];
        prepare_lines(&edges, &mut lines).unwrap();
        let (width, height) = (48, 18);
        let (mut segments, mut trapezoids, mut row_area, mut offsets, mut indices) =
            raster_workspaces(&lines, width, height);
        let mut raster_workspace = Workspace {
            segments: &mut segments, trapezoids: &mut trapezoids, row_area: &mut row_area,
            strip_offsets: &mut offsets, strip_indices: &mut indices,
        };

        let mut  tiles = [CoverageTile::default(); 8];
        let mut   runs = [CoverageTileRun::default(); 128];
        let mut pieces = [DirectTilePiece::default(); 128];
        let (mut heads, mut tails, mut touched) = ([0; 3], [0; 3], [0; 3]);

        assert_eq!(rasterize_lines_to_tiles(&lines, width, height, FillRule::NonZero,
            &mut raster_workspace, DirectTileWorkspace {
                tiles: &mut tiles, runs: &mut runs, pieces: &mut pieces,
                column_heads: &mut heads[..2], column_tails: &mut tails,
                touched_columns: &mut touched,
            }).unwrap_err(), Error::WorkspaceTooSmall {
            kind: WorkspaceKind::CoverageTileColumns, required: 3,
        });
        assert_eq!(rasterize_lines_to_tiles(&lines, width, height, FillRule::NonZero,
            &mut raster_workspace, DirectTileWorkspace {
                tiles: &mut tiles, runs: &mut runs, pieces: &mut [],
                column_heads: &mut heads, column_tails: &mut tails,
                touched_columns: &mut touched,
            }).unwrap_err(), Error::WorkspaceTooSmall {
            kind: WorkspaceKind::CoverageTilePieces, required: 1,
        });
        assert_eq!(rasterize_lines_to_tiles(&lines, width, height, FillRule::NonZero,
            &mut raster_workspace, DirectTileWorkspace {
                tiles: &mut [], runs: &mut runs, pieces: &mut pieces,
                column_heads: &mut heads, column_tails: &mut tails,
                touched_columns: &mut touched,
            }).unwrap_err(), Error::WorkspaceTooSmall {
            kind: WorkspaceKind::CoverageTiles, required: 2,
        });
        assert!(matches!(rasterize_lines_to_tiles(&lines, width, height,
            FillRule::NonZero, &mut raster_workspace, DirectTileWorkspace {
                tiles: &mut tiles, runs: &mut [], pieces: &mut pieces,
                column_heads: &mut heads, column_tails: &mut tails,
                touched_columns: &mut touched,
            }), Err(Error::WorkspaceTooSmall {
                kind: WorkspaceKind::CoverageTileRuns, required: 1..,
            })));
    }

    #[test] fn randomized_stream_strip_and_direct_tile_coverage_are_identical() {
        let (mut state, mut successful) = (0xA5A5_1F2E_39C4_D507_u64, 0);
        let mut random = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for case in 0..256 {
            let (width, height) = (1 + random() as u32 % 35, 1 + random() as u32 % 35);
            let mut edges = Vec::new();
            for _ in 0..1 + random() as usize % 3 {
                let count = 3 + random() as usize % 5;
                let mut points = Vec::with_capacity(count);
                for _ in 0..count {
                    let x = random() as i32 % ((width + 4) as i32 * 256) - 512;
                    let y = random() as i32 % ((height + 4) as i32 * 256) - 512;
                    points.push((Scalar::from_bits(x),
                                 Scalar::from_bits(y)).into());
                }
                for index in 0..count {
                    if let Some(edge) = Edge::from_line(points[index],
                        points[(index + 1) % count]) {
                        edges.push(edge);
                    }
                }
            }
            let mut lines = vec![Line::default(); edges.len()];
            let line_count = prepare_lines(&edges, &mut lines).unwrap();
            lines.truncate(line_count);
            let (mut segments, mut trapezoids, mut row_area, mut offsets, mut indices) =
                raster_workspaces(&lines, width, height);
            let requirements = requirements(width, height).unwrap();
            let mut strip_runs =
                vec![CoverageRun::default(); width as usize * height as usize];
            let mut strips = vec![CoverageStrip::default();
                    height.div_ceil(STRIP_HEIGHT) as usize];
            let mut tiles  = vec![CoverageTile::default(); requirements.tiles];
            let mut pieces = vec![DirectTilePiece::default(); requirements.pieces];
            let mut tile_runs = vec![CoverageTileRun::default(); requirements.runs];
            let (mut heads, mut tails, mut touched) = (vec![0; requirements.columns],
                vec![0; requirements.columns],    vec![0; requirements.columns],
            );
            for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
                let mut workspace = Workspace {
                    segments: &mut segments, trapezoids: &mut trapezoids,
                    row_area: &mut row_area, strip_offsets: &mut offsets,
                    strip_indices: &mut indices,
                };
                let mut streamed = vec![0; width as usize * height as usize];
                let result = rasterize_lines(&lines, width, height, fill_rule, &mut workspace,
                    &mut |x, y, coverage| {
                        streamed[y as usize * width as usize + x as usize] = coverage;
                        Ok::<_, Infallible>(())
                    });
                if let Err(error) = result {
                    let RenderError::Raster(error) = error;
                    assert_eq!(rasterize_lines_to_strips(&lines, width, height, fill_rule,
                        &mut workspace, CoverageWorkspace {
                            strips: &mut strips, runs: &mut strip_runs,
                        }).unwrap_err(), error,
                        "strip error mismatch in case {case}, {fill_rule:?}");
                    assert_eq!(rasterize_lines_to_tiles(&lines, width, height, fill_rule,
                        &mut workspace, DirectTileWorkspace {
                            tiles: &mut tiles, runs: &mut tile_runs, pieces: &mut pieces,
                            column_heads: &mut heads, column_tails: &mut tails,
                            touched_columns: &mut touched,
                        }).unwrap_err(), error,
                        "tile error mismatch in case {case}, {fill_rule:?}");
                    continue;
                }
                successful += 1;

                let retained = rasterize_lines_to_strips(&lines, width, height, fill_rule,
                    &mut workspace, CoverageWorkspace {
                        strips: &mut strips, runs: &mut strip_runs,
                    }).unwrap();
                let mut replayed_strips = vec![0; streamed.len()];
                retained.replay(&mut |x, y, coverage| {
                    replayed_strips[y as usize * width as usize + x as usize] = coverage;
                    Ok::<_, Infallible>(())
                }).unwrap();
                assert_eq!(replayed_strips, streamed,
                    "strip mismatch in case {case}, {fill_rule:?}");

                let tiled = rasterize_lines_to_tiles(&lines, width, height, fill_rule,
                    &mut workspace, DirectTileWorkspace {
                        tiles: &mut tiles, runs: &mut tile_runs, pieces: &mut pieces,
                        column_heads: &mut heads, column_tails: &mut tails,
                        touched_columns: &mut touched,
                    }).unwrap();
                let mut replayed_tiles = vec![0; streamed.len()];
                tiled.replay(&mut |x, y, coverage| {
                    replayed_tiles[y as usize * width as usize + x as usize] = coverage;
                    Ok::<_, Infallible>(())
                }).unwrap();
                assert_eq!(replayed_tiles, streamed,
                    "tile mismatch in case {case}, {fill_rule:?}");
            }
        }
        assert!(successful >= 256, "only {successful} randomized pipelines completed");
    }
}
