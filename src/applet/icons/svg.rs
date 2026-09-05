use std::ffi::OsStr;
use std::path::Path;

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

use crate::core::icons::RgbaImage;

use super::paint::{MIN_ALPHA, MIN_LIGHTNESS_SPAN, lightness_of, straighten};

const MAX_SVG_BYTES: u64 = 256 * 1024;

const INSPECT_SIZE: u16 = 32;

const MAX_CHROMA: u8 = 8;

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

pub fn single_ink_svg(path: &Path) -> bool {
    if path.extension() != Some(OsStr::new("svg"))
        || std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_SVG_BYTES)
    {
        return false;
    }

    let Ok(source) = std::fs::read_to_string(path) else {
        return false;
    };

    if source.contains("<image") || source.contains("data:image/") {
        return false;
    }

    rendered_ink(source.as_bytes()).is_none_or(|ink| ink.is_single())
}

struct RenderedInk {
    span: f32,
    chroma: u8,
}

impl RenderedInk {
    fn is_single(&self) -> bool {
        self.chroma <= MAX_CHROMA && self.span < MIN_LIGHTNESS_SPAN
    }
}

fn rendered_ink(source: &[u8]) -> Option<RenderedInk> {
    let tree = usvg::Tree::from_data(source, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = f32::from(INSPECT_SIZE) / size.width().max(size.height()).max(1.0);
    let mut pixmap = Pixmap::new(u32::from(INSPECT_SIZE), u32::from(INSPECT_SIZE))?;
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let mut span: Option<(f32, f32)> = None;
    let mut chroma = 0u8;
    for pixel in pixmap.pixels() {
        let alpha = pixel.alpha();
        if alpha < MIN_ALPHA {
            continue;
        }
        let colour = [
            straighten(pixel.red(), alpha),
            straighten(pixel.green(), alpha),
            straighten(pixel.blue(), alpha),
        ];
        let low = colour[0].min(colour[1]).min(colour[2]);
        let high = colour[0].max(colour[1]).max(colour[2]);
        chroma = chroma.max(high - low);

        let level = lightness_of(colour);
        span = Some(match span {
            Some((low, high)) => (low.min(level), high.max(level)),
            None => (level, level),
        });
    }

    span.map(|(low, high)| RenderedInk {
        span: high - low,
        chroma,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::applet::icons::testing::*;

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
    fn a_single_ink_svg_is_symbolic_even_without_the_suffix() {
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
                single_ink_svg(&svg_at(&root, name, &body)),
                "expected {name} to be treated as symbolic"
            );
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn two_greyscale_inks_are_preserved() {
        let root = test_root("two-inks");
        let path = svg_at(
            &root,
            "outlined",
            &drawn(
                "<rect width=\"8\" height=\"16\" fill=\"#111111\"/>\
                 <rect x=\"8\" width=\"8\" height=\"16\" fill=\"#eeeeee\"/>",
            ),
        );

        assert!(!single_ink_svg(&path));
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

        let painted = super::super::paint::prepare_raster(&rendered, 24, &dark_panel())
            .expect("the vector is painted");
        let pixels = painted.bytes.as_chunks::<4>().0;
        let base = pixels[12 * 24 + 8];
        let badge = pixels[4 * 24 + 20];

        assert_eq!(&base[..3], &dark_panel().ink);
        assert!(badge[0].saturating_sub(badge[2]) > 60);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unused_coloured_style_rule_does_not_change_the_live_single_ink() {
        let root = test_root("dead-rules");
        let body = drawn(
            "<style>.ColorScheme-Text{color:#dfdfdf}.ColorScheme-Highlight{color:#4285f4}</style>\
             <rect class=\"ColorScheme-Text\" width=\"16\" height=\"16\" fill=\"currentColor\"/>",
        );

        assert!(single_ink_svg(&svg_at(&root, "unused-highlight", &body)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_live_coloured_style_rule_keeps_the_svg_original() {
        let root = test_root("live-rules");
        let body = drawn(
            "<style>.ColorScheme-Text{color:#dfdfdf}.ColorScheme-Highlight{color:#4285f4}</style>\
             <rect class=\"ColorScheme-Highlight\" width=\"16\" height=\"16\" fill=\"currentColor\"/>",
        );

        assert!(!single_ink_svg(&svg_at(&root, "used-highlight", &body)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn coloured_or_embedded_content_is_not_inferred_as_single_ink() {
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
                !single_ink_svg(&svg_at(&root, name, &body)),
                "expected {name} not to be inferred as single ink"
            );
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn artwork_that_draws_nothing_is_left_to_the_theme_ink() {
        let root = test_root("empty-vector");

        assert!(single_ink_svg(&svg_at(&root, "blank", "<svg/>")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_raster_file_is_never_inferred_as_symbolic() {
        let root = test_root("raster");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tray.png");
        std::fs::write(&path, [0u8; 8]).unwrap();

        assert!(!single_ink_svg(&path));
        std::fs::remove_dir_all(root).unwrap();
    }
}
