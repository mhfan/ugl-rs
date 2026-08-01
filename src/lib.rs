
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod common;
pub mod shader;     // reserved for a future optional 3D layer

#[cfg(feature = "f32")]   pub mod float;
#[cfg(feature = "fixed")] pub mod fixed;

/// Canvas for the default enabled backend (`f32` takes precedence when both exist).
#[cfg(feature = "f32")] pub use float::Canvas;
#[cfg(all(feature = "fixed", not(feature = "f32")))] pub use fixed::Canvas;

#[cfg(all(test, feature = "f32", feature = "fixed"))] mod facade_tests {
    use super::{common::{color::SRGBA, geometry::{Affine, Path, PathBuilder, Rect}},
        fixed::{self, Scalar}, float};

    fn float_paths() -> (Path, Path, Path) {
        let mut fill = PathBuilder::new();
        fill.move_to((0.0, 0.0)).line_to((4.0, 0.0))
            .line_to((4.0, 3.0)).line_to((0.0, 3.0));
        let mut stroke = PathBuilder::new();
        stroke.move_to((0.0, 4.0)).line_to((6.0, 4.0));
        let mut clip = PathBuilder::new();
        clip.move_to((1.0, 0.0)).line_to((5.0, 0.0))
            .line_to((5.0, 6.0)).line_to((1.0, 6.0));
        (fill.build(), stroke.build(), clip.build())
    }

    fn fixed_paths() -> (Path<Scalar>, Path<Scalar>, Path<Scalar>) {
        let fixed = Scalar::from_num;
        let mut fill = PathBuilder::new();
        fill.move_to((fixed(0), fixed(0))).line_to((fixed(4), fixed(0)))
            .line_to((fixed(4), fixed(3))).line_to((fixed(0), fixed(3)));
        let mut stroke = PathBuilder::new();
        stroke.move_to((fixed(0), fixed(4))).line_to((fixed(6), fixed(4)));
        let mut clip = PathBuilder::new();
        clip.move_to((fixed(1), fixed(0))).line_to((fixed(5), fixed(0)))
            .line_to((fixed(5), fixed(6))).line_to((fixed(1), fixed(6)));
        (fill.build(), stroke.build(), clip.build())
    }

    fn render_float(path_clip: bool) -> Vec<u8> {
        let (fill, stroke, clip) = float_paths();
        let mut canvas = float::Canvas::new(6, 6).unwrap();
        canvas.set_color(SRGBA::new(220, 40, 80, 192))
            .set_global_alpha(224)
            .set_transform(Affine::translate(1.0, 0.0));
        if path_clip { canvas.set_clip_path(&clip).unwrap(); }
        else { canvas.set_clip_rect(Rect::from_ltrb(1.0, 0.0, 5.0, 6.0).unwrap()); }
        canvas.save().fill(&fill).unwrap();
        canvas.set_transform(Affine::identity())
            .set_stroke(float::stroke::StrokeOptions::new(2.0).unwrap())
            .stroke(&stroke).unwrap();
        canvas.restore();
        canvas.stroke_dashed(&stroke,
            float::dash::DashPattern::new(&[2.0, 1.0], 0.0).unwrap()).unwrap();
        canvas.target().as_bytes().to_vec()
    }

    fn render_fixed(path_clip: bool) -> Vec<u8> {
        let (fill, stroke, clip) = fixed_paths();
        let fixed = Scalar::from_num;
        let mut canvas = fixed::Canvas::new(6, 6).unwrap();
        canvas.set_color(SRGBA::new(220, 40, 80, 192))
            .set_global_alpha(224)
            .set_transform(Affine::translate(fixed(1), Scalar::ZERO));
        if path_clip { canvas.set_clip_path(&clip).unwrap(); }
        else { canvas.set_clip_rect(Rect::from_ltrb(
            fixed(1), fixed(0), fixed(5), fixed(6)).unwrap()); }
        canvas.save().fill(&fill).unwrap();
        canvas.set_transform(Affine::identity())
            .set_stroke(fixed::stroke::Options::new(fixed(2)).unwrap())
            .stroke(&stroke).unwrap();
        canvas.restore();
        canvas.stroke_dashed(&stroke,
            fixed::dash::Pattern::new(&[fixed(2), fixed(1)], Scalar::ZERO).unwrap()).unwrap();
        canvas.target().as_bytes().to_vec()
    }

    #[test] fn backend_facades_match_shared_state_and_rectangle_clip() {
        assert_eq!(render_fixed(false), render_float(false));
    }

    #[test] fn backend_facades_match_owned_path_clip_and_restore() {
        assert_eq!(render_fixed(true), render_float(true));
    }
}
