use std::ffi::OsStr;
use std::path::Path;

use cosmic::widget::icon;
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

use super::paint::{MAX_TINT_SHIFT, MIN_ALPHA, MIN_LIGHTNESS_SPAN, ink_is_light, lightness_of};

const MAX_SVG_BYTES: u64 = 256 * 1024;

const INSPECT_SIZE: u16 = 32;

const MAX_CHROMA: u8 = 8;

pub fn tinted_svg(path: &Path, ink: [u8; 3]) -> Option<icon::Handle> {
    if !std::fs::metadata(path).is_ok_and(|meta| meta.len() <= MAX_SVG_BYTES) {
        return None;
    }
    let mut source = std::fs::read_to_string(path).ok()?;
    let svg = source.find("<svg")?;
    let opening = svg + source[svg..].find('>')? + 1;
    let closing = source.rfind("</svg>")?;
    if closing < opening {
        return None;
    }

    let intercept = tint_intercepts(ink);
    let red = 0.2126 * MAX_TINT_SHIFT;
    let green = 0.7152 * MAX_TINT_SHIFT;
    let blue = 0.0722 * MAX_TINT_SHIFT;
    let filter = format!(
        "<defs><filter id=\"status-hub-theme-tint\" x=\"-10%\" y=\"-10%\" width=\"120%\" height=\"120%\" color-interpolation-filters=\"sRGB\"><feColorMatrix type=\"matrix\" values=\"{red} {green} {blue} 0 {} {red} {green} {blue} 0 {} {red} {green} {blue} 0 {} 0 0 0 1 0\"/></filter></defs><g filter=\"url(#status-hub-theme-tint)\">",
        intercept[0], intercept[1], intercept[2]
    );
    source.insert_str(closing, "</g>");
    source.insert_str(opening, &filter);
    Some(icon::from_svg_bytes(source.into_bytes()))
}

fn tint_intercepts(ink: [u8; 3]) -> [f32; 3] {
    let anchor = if ink_is_light(ink) {
        MAX_TINT_SHIFT
    } else {
        0.0
    };
    ink.map(|channel| f32::from(channel) / 255.0 - anchor)
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
        let straighten = |channel: u8| {
            u8::try_from((u16::from(channel) * 255 / u16::from(alpha)).min(255)).unwrap_or(u8::MAX)
        };
        let colour = [
            straighten(pixel.red()),
            straighten(pixel.green()),
            straighten(pixel.blue()),
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
    fn a_tinted_vector_shades_the_ink_without_reaching_the_panel() {
        for (ink, base) in [
            (dark_panel().ink, DARK_PANEL_BASE),
            (light_panel().ink, LIGHT_PANEL_BASE),
        ] {
            let intercepts = tint_intercepts(ink);
            for source in [0.0, 0.5, 1.0] {
                for channel in 0..3 {
                    let level = f32::from(ink[channel]) / 255.0;
                    let towards_panel = (f32::from(base[channel]) / 255.0 - level).signum();
                    let travel =
                        (MAX_TINT_SHIFT * source + intercepts[channel] - level) * towards_panel;
                    assert!(travel >= -f32::EPSILON);
                    assert!(travel <= MAX_TINT_SHIFT + f32::EPSILON);
                }
            }
        }
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
