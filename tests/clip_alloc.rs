#![cfg(feature = "fixed")]

use std::{alloc::{GlobalAlloc, Layout, System}, sync::atomic::{AtomicUsize, Ordering}};
use ugl_rs::{common::{color::SRGBA, geometry::PathBuilder, raster::CoverageMask},
    fixed::{Canvas, Scalar}};

struct CountingAllocator;
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

fn record_allocation(size: usize) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    BYTES.fetch_add(size, Ordering::Relaxed);
    let current = CURRENT.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(current, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() { record_allocation(layout.size()); }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() { record_allocation(layout.size()); }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout); }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if !resized.is_null() {
            CALLS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size, Ordering::Relaxed);
            let current = if new_size >= layout.size() {
                CURRENT.fetch_add(new_size - layout.size(), Ordering::Relaxed) +
                    new_size - layout.size()
            } else {
                CURRENT.fetch_sub(layout.size() - new_size, Ordering::Relaxed) -
                    (layout.size() - new_size)
            };
            PEAK.fetch_max(current, Ordering::Relaxed);
        }
        resized
    }
}

#[global_allocator] static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy, Debug)] struct AllocationStats {
    calls: usize, bytes: usize, peak_bytes: usize,
}

fn measure<T>(operation: impl FnOnce() -> T) -> (T, AllocationStats) {
    let baseline = CURRENT.load(Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let result = operation();
    let stats = AllocationStats {
        calls: CALLS.load(Ordering::Relaxed), bytes: BYTES.load(Ordering::Relaxed),
        peak_bytes: PEAK.load(Ordering::Relaxed).saturating_sub(baseline),
    };
    (result, stats)
}

#[test] fn retained_sparse_clip_has_bounded_peak_and_zero_allocation_warm_draws() {
    const SIZE: usize = 512;
    let mut coverage = vec![0; SIZE * SIZE];
    for y in 0..SIZE { coverage[y * SIZE + y] = 128; }
    let mask = CoverageMask::new(&coverage, SIZE as _, SIZE as _, SIZE as _).unwrap();
    let mut canvas = Canvas::new(SIZE as _, SIZE as _).unwrap();
    let (_, retain) = measure(|| { canvas.set_clip_mask(mask); });

    let fixed = Scalar::from_num;
    let mut shape = PathBuilder::new();
    shape.move_to((fixed(0), fixed(0))).line_to((fixed(SIZE), fixed(0)))
        .line_to((fixed(SIZE), fixed(SIZE))).line_to((fixed(0), fixed(SIZE)));
    let shape = shape.build();
    canvas.set_color(SRGBA::red()).fill(&shape).unwrap();
    let (_, warm_draw) = measure(|| {
        canvas.target_mut().as_bytes_mut().fill(0);
        canvas.fill(&shape).unwrap();
    });

    canvas.save(); assert!(canvas.restore());
    let (_, saved_state) = measure(|| { canvas.save(); assert!(canvas.restore()); });
    let rect = ugl_rs::common::geometry::Rect::from_ltrb(
        Scalar::from_num(128.5), Scalar::from_num(128.5),
        Scalar::from_num(383.5), Scalar::from_num(383.5)).unwrap();
    let (_, intersection) = measure(|| { canvas.set_clip_rect(rect); });

    const DENSE_SIZE: usize = 64;
    let mut dense = vec![0; DENSE_SIZE * DENSE_SIZE];
    for y in 0..DENSE_SIZE { for x in 0..DENSE_SIZE {
        dense[y * DENSE_SIZE + x] = if (x + y) & 1 == 0 { 96 } else { 192 };
    } }
    let mut dense_canvas = Canvas::new(DENSE_SIZE as _, DENSE_SIZE as _).unwrap();
    dense_canvas.set_clip_mask(CoverageMask::new(
        &dense, DENSE_SIZE as _, DENSE_SIZE as _, DENSE_SIZE as _).unwrap());
    dense_canvas.save();
    let dense_rect = ugl_rs::common::geometry::Rect::from_ltrb(
        Scalar::from_num(8.5), Scalar::from_num(8.5),
        Scalar::from_num(55.5), Scalar::from_num(55.5)).unwrap();
    let (_, dense_cow) = measure(|| { dense_canvas.set_clip_rect(dense_rect); });
    assert!(dense_canvas.restore());

    let mut slender = PathBuilder::new();
    slender.move_to((fixed(0), fixed(8))).line_to((fixed(504), fixed(512)))
        .line_to((fixed(512), fixed(504))).line_to((fixed(8), fixed(0)));
    let mut path_canvas = Canvas::new(SIZE as _, SIZE as _).unwrap();
    let (_, path_clip) = measure(|| { path_canvas.set_clip_path(&slender.build()).unwrap(); });

    eprintln!("clip allocation stats: retain={retain:?}, warm_draw={warm_draw:?}, \
        save_restore={saved_state:?}, sparse_rect={intersection:?}, dense_cow={dense_cow:?}, \
        sparse_path={path_clip:?}");
    assert!(retain.peak_bytes < SIZE * SIZE);
    assert_eq!((warm_draw.calls, warm_draw.bytes, warm_draw.peak_bytes), (0, 0, 0));
    assert_eq!((saved_state.calls, saved_state.bytes, saved_state.peak_bytes), (0, 0, 0));
    assert!(intersection.peak_bytes < SIZE * SIZE);
    assert!(dense_cow.peak_bytes <= DENSE_SIZE * DENSE_SIZE * 3);
    assert!(path_clip.peak_bytes < SIZE * SIZE);
}
