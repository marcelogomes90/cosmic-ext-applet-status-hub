use std::path::Path;

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

use crate::core::icons::RgbaImage;

use super::raster::straighten;

pub(super) const MAX_SVG_BYTES: u64 = 256 * 1024;

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn render_svg(path: &Path, size: u16) -> Option<RgbaImage> {
    if !std::fs::metadata(path).is_ok_and(|meta| meta.len() <= MAX_SVG_BYTES) {
        return None;
    }
    let source = std::fs::read(path).ok()?;
    let tree = usvg::Tree::from_data(&source, &usvg::Options::default()).ok()?;
    let tree_size = tree.size();
    let target = f32::from(size.max(1).saturating_mul(2));
    let scale = target / tree_size.width().max(tree_size.height()).max(1.0);
    let width = (tree_size.width() * scale).ceil().max(1.0) as u32;
    let height = (tree_size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let mut bytes = pixmap.data().to_vec();
    for pixel in bytes.as_chunks_mut::<4>().0 {
        let alpha = pixel[3];
        for channel in &mut pixel[..3] {
            *channel = straighten(*channel, alpha);
        }
    }

    Some(RgbaImage {
        width,
        height,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::applet::icons::testing::*;

    fn is_monotone(path: &Path) -> bool {
        render_svg(path, 16).is_some_and(|mut image| {
            super::super::paint::recolour(&mut image, &light_panel(), false)
                == super::super::paint::PaintDecision::Monotone
        })
    }

    fn drawn(body: &str) -> String {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" width=\"16\" height=\"16\">{body}</svg>"
        )
    }

    fn filled(paint: &str) -> String {
        drawn(&format!(
            "<rect width=\"16\" height=\"16\" fill=\"{paint}\"/>"
        ))
    }

    #[test]
    fn rendered_neutral_fills_use_the_shared_monotone_classifier() {
        let root = test_root("single-ink");
        let cases = [
            ("black", filled("#000000")),
            ("current", filled("currentColor")),
            (
                "css",
                drawn(
                    "<style>.a{fill:#222222}</style><rect class=\"a\" width=\"16\" height=\"16\"/>",
                ),
            ),
            ("functional", filled("rgb(40, 40, 40)")),
            ("tinted-grey", filled("#232629")),
            ("percentages", filled("rgb(20%,20%,20%)")),
            (
                "equivalent",
                drawn(
                    "<rect width=\"8\" height=\"16\" fill=\"#222222\"/>\
                     <rect x=\"8\" width=\"8\" height=\"16\" fill=\"rgb(34,34,34)\"/>",
                ),
            ),
        ];

        for (name, body) in cases {
            assert!(
                is_monotone(&svg_at(&root, name, &body)),
                "expected {name} to be treated as monotone"
            );
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn two_neutral_svg_regions_use_the_shared_duotone_classifier() {
        let root = test_root("two-inks");
        let path = svg_at(
            &root,
            "outlined",
            &drawn(
                "<rect width=\"8\" height=\"16\" fill=\"#111111\"/>\
                 <rect x=\"8\" width=\"8\" height=\"16\" fill=\"#eeeeee\"/>",
            ),
        );

        let mut image = render_svg(&path, 16).unwrap();
        assert!(matches!(
            super::super::paint::recolour(&mut image, &light_panel(), false),
            super::super::paint::PaintDecision::Duotone { .. }
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_detailed_vector_is_rendered_at_twice_the_requested_size() {
        let root = test_root("rendered-vector");
        let path = svg_at(
            &root,
            "wide",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 8\"><rect width=\"16\" height=\"8\" fill=\"red\"/></svg>",
        );

        let image = render_svg(&path, 24).expect("the vector renders");

        assert_eq!((image.width, image.height), (48, 24));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_vector_badge_keeps_its_accent_through_the_raster_painter() {
        let root = test_root("vector-badge");
        let path = svg_at(
            &root,
            "badge",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\">\
             <rect x=\"3\" y=\"3\" width=\"18\" height=\"18\" rx=\"3\" fill=\"white\"/>\
             <circle cx=\"20\" cy=\"4\" r=\"4\" fill=\"#e01e5a\"/></svg>",
        );
        let rendered = render_svg(&path, 24).expect("the vector renders");

        let mut painted = rendered;
        assert_eq!(
            super::super::paint::recolour(&mut painted, &dark_panel(), false),
            super::super::paint::PaintDecision::Monotone,
        );
        let painted = super::super::raster::prepare(painted, 24).unwrap();
        let pixels = painted.bytes.as_chunks::<4>().0;
        assert_eq!((painted.width, painted.height), (48, 48));
        let base = pixels[24 * 48 + 16];
        let badge = pixels[8 * 48 + 40];

        assert_eq!(&base[..3], &dark_panel().ink);
        assert_eq!(badge, [224, 30, 90, 255]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unused_coloured_style_rule_does_not_change_the_live_single_ink() {
        let root = test_root("dead-rules");
        let body = drawn(
            "<style>.ColorScheme-Text{color:#dfdfdf}.ColorScheme-Highlight{color:#4285f4}</style>\
             <rect class=\"ColorScheme-Text\" width=\"16\" height=\"16\" fill=\"currentColor\"/>",
        );

        assert!(is_monotone(&svg_at(&root, "unused-highlight", &body)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_live_coloured_style_rule_keeps_the_svg_original() {
        let root = test_root("live-rules");
        let body = drawn(
            "<style>.ColorScheme-Text{color:#dfdfdf}.ColorScheme-Highlight{color:#4285f4}</style>\
             <rect class=\"ColorScheme-Highlight\" width=\"16\" height=\"16\" fill=\"currentColor\"/>",
        );

        assert!(!is_monotone(&svg_at(&root, "used-highlight", &body)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn colours_gradients_and_invalid_embedded_content_are_not_monotone() {
        let root = test_root("coloured");
        let cases = [
            ("hex", filled("#4caf50")),
            ("named", filled("red")),
            (
                "gradient",
                drawn(
                    "<defs><linearGradient id=\"g\"><stop stop-color=\"#000000\"/>\
                     <stop offset=\"1\" stop-color=\"#ffffff\"/></linearGradient></defs>\
                     <rect width=\"16\" height=\"16\" fill=\"url(#g)\"/>",
                ),
            ),
            (
                "mixed",
                drawn(
                    "<rect width=\"8\" height=\"16\" fill=\"#000000\"/>\
                     <rect x=\"8\" width=\"8\" height=\"16\" fill=\"#0088ff\"/>",
                ),
            ),
            (
                "raster",
                drawn("<image href=\"data:image/png;base64,AAA\" width=\"16\" height=\"16\"/>"),
            ),
        ];

        for (name, body) in cases {
            assert!(
                !is_monotone(&svg_at(&root, name, &body)),
                "expected {name} not to be inferred as single ink"
            );
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_empty_vector_is_not_classified_as_monotone() {
        let root = test_root("empty-vector");

        assert!(!is_monotone(&svg_at(&root, "blank", "<svg/>")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_svg_renderer_rejects_non_svg_bytes() {
        let root = test_root("raster");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tray.png");
        std::fs::write(&path, [0u8; 8]).unwrap();

        assert!(!is_monotone(&path));
        std::fs::remove_dir_all(root).unwrap();
    }
}
