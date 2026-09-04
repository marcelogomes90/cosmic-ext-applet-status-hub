use std::borrow::Cow;

use crate::core::icons::RgbaImage;

use super::ThemeContext;

pub const MIN_ALPHA: u8 = 16;

const MIN_BADGE_PIXEL_CHROMA: u8 = 16;

const MIN_CLEAR_PERCENT: usize = 10;

const MAX_BADGE_BOX_PERCENT: usize = 40;

const MAX_BADGE_VISIBLE_PERCENT: usize = 50;

const MIN_BADGE_FILL_PERCENT: usize = 15;

const BADGE_ORIGINAL_WEIGHT: f32 = 0.8;

pub const MIN_LIGHTNESS_SPAN: f32 = 0.03;

pub const MAX_TINT_SHIFT: f32 = 0.34;

const TINT_GAIN: f32 = 0.85;

fn recolour(image: &RgbaImage, theme: &ThemeContext) -> Option<RgbaImage> {
    let width = usize::try_from(image.width).ok()?;
    let height = usize::try_from(image.height).ok()?;
    let pixels = image.bytes.as_chunks::<4>().0;
    if width == 0 || height == 0 || pixels.len() != width * height {
        return None;
    }

    let badge = classify_badge(pixels, width, height);
    let tones = tone_profile(pixels, width, badge.as_ref(), theme.ink)?;
    let mut bytes = image.bytes.clone();
    let mut painted = false;
    for (index, pixel) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = index % width;
        let y = index / width;
        if pixel[3] > 0 {
            let alpha = pixel[3];
            let colour = if badge.as_ref().is_some_and(|badge| badge.contains(x, y)) {
                badge_colour(*pixel, theme.ink)
            } else {
                tones.tint(*pixel)
            };
            *pixel = [colour[0], colour[1], colour[2], alpha];
            painted = true;
        }
    }

    painted.then_some(RgbaImage {
        width: image.width,
        height: image.height,
        bytes,
    })
}

struct ToneProfile {
    ink: [u8; 3],
    tint: Oklab,
    low: f32,
    high: f32,
    anchor: f32,
    gain: f32,
    detailed: bool,
}

impl ToneProfile {
    fn tint(&self, pixel: [u8; 4]) -> [u8; 3] {
        if !self.detailed {
            return self.ink;
        }
        let difference = luminance(pixel).clamp(self.low, self.high) - self.anchor;
        oklab_to_srgb(Oklab {
            lightness: (self.tint.lightness + difference * self.gain).clamp(0.0, 1.0),
            ..self.tint
        })
    }
}

fn tone_profile(
    pixels: &[[u8; 4]],
    width: usize,
    badge: Option<&Badge>,
    ink: [u8; 3],
) -> Option<ToneProfile> {
    let peak = pixels
        .iter()
        .enumerate()
        .filter(|&(index, _)| {
            badge.is_none_or(|badge| !badge.contains(index % width, index / width))
        })
        .map(|(_, pixel)| pixel[3])
        .max()?;
    let mut levels: Vec<f32> = pixels
        .iter()
        .enumerate()
        .filter(|(index, pixel)| {
            pixel[3] >= MIN_ALPHA
                && u16::from(pixel[3]) * 10 >= u16::from(peak) * 9
                && badge.is_none_or(|badge| !badge.contains(index % width, index / width))
        })
        .map(|(_, &pixel)| luminance(pixel))
        .collect();
    if levels.is_empty() {
        return None;
    }
    levels.sort_unstable_by(f32::total_cmp);
    let margin = levels.len() / 20;
    let low = levels[margin];
    let high = levels[levels.len() - 1 - margin];

    let span = high - low;
    let tint = srgb_to_oklab(ink);
    let dark_theme = tint.is_light();
    let anchor = if dark_theme { high } else { low };
    let room = if dark_theme {
        tint.lightness
    } else {
        1.0 - tint.lightness
    };
    let gain = TINT_GAIN
        .min(MAX_TINT_SHIFT / span.max(f32::EPSILON))
        .min(room / span.max(f32::EPSILON));

    Some(ToneProfile {
        ink,
        tint,
        low,
        high,
        anchor,
        gain,
        detailed: span >= MIN_LIGHTNESS_SPAN,
    })
}

fn badge_colour(pixel: [u8; 4], ink: [u8; 3]) -> [u8; 3] {
    let original = srgb_to_oklab([pixel[0], pixel[1], pixel[2]]);
    let themed = srgb_to_oklab(ink);
    oklab_to_srgb(Oklab {
        lightness: original.lightness * BADGE_ORIGINAL_WEIGHT
            + themed.lightness * (1.0 - BADGE_ORIGINAL_WEIGHT),
        a: original.a * BADGE_ORIGINAL_WEIGHT + themed.a * (1.0 - BADGE_ORIGINAL_WEIGHT),
        b: original.b * BADGE_ORIGINAL_WEIGHT + themed.b * (1.0 - BADGE_ORIGINAL_WEIGHT),
    })
}

#[derive(Clone, Copy)]
struct Oklab {
    lightness: f32,
    a: f32,
    b: f32,
}

impl Oklab {
    fn is_light(self) -> bool {
        self.lightness >= 0.5
    }
}

pub fn lightness_of(rgb: [u8; 3]) -> f32 {
    srgb_to_oklab(rgb).lightness
}

pub fn ink_is_light(ink: [u8; 3]) -> bool {
    srgb_to_oklab(ink).is_light()
}

fn luminance(pixel: [u8; 4]) -> f32 {
    srgb_to_oklab([pixel[0], pixel[1], pixel[2]]).lightness
}

fn srgb_to_oklab(rgb: [u8; 3]) -> Oklab {
    let [red, green, blue] = rgb.map(srgb_to_linear);
    let l = 0.412_221_46 * red + 0.536_332_55 * green + 0.051_445_995 * blue;
    let m = 0.211_903_5 * red + 0.680_699_5 * green + 0.107_396_96 * blue;
    let s = 0.088_302_46 * red + 0.281_718_85 * green + 0.629_978_7 * blue;
    let [l, m, s] = [l.cbrt(), m.cbrt(), s.cbrt()];
    Oklab {
        lightness: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    }
}

fn oklab_to_srgb(colour: Oklab) -> [u8; 3] {
    let l = colour.lightness + 0.396_337_78 * colour.a + 0.215_803_76 * colour.b;
    let m = colour.lightness - 0.105_561_346 * colour.a - 0.063_854_17 * colour.b;
    let s = colour.lightness - 0.089_484_18 * colour.a - 1.291_485_5 * colour.b;
    let [l, m, s] = [l * l * l, m * m * m, s * s * s];
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
    .map(linear_to_srgb)
}

fn srgb_to_linear(channel: u8) -> f32 {
    let channel = f32::from(channel) / 255.0;
    if channel <= 0.040_45 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn linear_to_srgb(channel: f32) -> u8 {
    let channel = if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    };
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

struct Badge {
    mask: Vec<bool>,
    width: usize,
}

impl Badge {
    fn contains(&self, x: usize, y: usize) -> bool {
        self.mask[y * self.width + x]
    }
}

fn classify_badge(pixels: &[[u8; 4]], width: usize, height: usize) -> Option<Badge> {
    let mut clear = 0usize;
    let mut visible = 0usize;
    let mut bounds: Option<Bounds> = None;
    let mut badge_coloured = 0usize;
    let mut rows = vec![None; height];
    let mut columns = vec![None; width];

    for (index, &pixel) in pixels.iter().enumerate() {
        if pixel[3] < MIN_ALPHA {
            clear += 1;
            continue;
        }

        let low = pixel[0].min(pixel[1]).min(pixel[2]);
        let high = pixel[0].max(pixel[1]).max(pixel[2]);
        let chroma = high - low;
        if chroma > MIN_BADGE_PIXEL_CHROMA {
            badge_coloured += 1;
            let x = index % width;
            let y = index / width;
            bounds = Some(match bounds {
                Some(bounds) => bounds.include(x, y),
                None => Bounds::new(x, y),
            });
            rows[y] = Some(include_span(rows[y], x));
            columns[x] = Some(include_span(columns[x], y));
        }
        visible += 1;
    }

    if visible == 0 || clear * 100 < pixels.len() * MIN_CLEAR_PERCENT {
        return None;
    }

    let bounds = bounds?;
    if plausible_badge(bounds, badge_coloured, visible, width, height) {
        let mut mask = vec![false; pixels.len()];
        for y in 0..height {
            for x in 0..width {
                mask[y * width + x] = rows[y].is_some_and(|(min, max)| (min..=max).contains(&x))
                    && columns[x].is_some_and(|(min, max)| (min..=max).contains(&y));
            }
        }
        let core = mask.clone();
        for (index, included) in core.into_iter().enumerate() {
            if !included {
                continue;
            }
            let x = index % width;
            let y = index / width;
            let min_x = x.saturating_sub(1);
            let max_x = x.saturating_add(1).min(width - 1);
            let min_y = y.saturating_sub(1);
            let max_y = y.saturating_add(1).min(height - 1);
            for neighbour_y in min_y..=max_y {
                for neighbour_x in min_x..=max_x {
                    let neighbour = neighbour_y * width + neighbour_x;
                    if pixels[neighbour][3] > 0 {
                        mask[neighbour] = true;
                    }
                }
            }
        }
        Some(Badge { mask, width })
    } else {
        None
    }
}

fn include_span(span: Option<(usize, usize)>, value: usize) -> (usize, usize) {
    span.map_or((value, value), |(min, max)| {
        (min.min(value), max.max(value))
    })
}

#[derive(Clone, Copy)]
struct Bounds {
    min_x: usize,
    min_y: usize,
    max_x: usize,
    max_y: usize,
}

impl Bounds {
    fn new(x: usize, y: usize) -> Self {
        Self {
            min_x: x,
            min_y: y,
            max_x: x,
            max_y: y,
        }
    }

    fn include(self, x: usize, y: usize) -> Self {
        Self {
            min_x: self.min_x.min(x),
            min_y: self.min_y.min(y),
            max_x: self.max_x.max(x),
            max_y: self.max_y.max(y),
        }
    }

    fn width(self) -> usize {
        self.max_x - self.min_x + 1
    }

    fn height(self) -> usize {
        self.max_y - self.min_y + 1
    }
}

fn plausible_badge(
    bounds: Bounds,
    coloured: usize,
    visible: usize,
    width: usize,
    height: usize,
) -> bool {
    let area = bounds.width() * bounds.height();
    let edge_margin = width.max(height).div_ceil(16).max(1);
    let at_edge = bounds.min_x <= edge_margin
        || bounds.min_y <= edge_margin
        || bounds.max_x.saturating_add(edge_margin) >= width - 1
        || bounds.max_y.saturating_add(edge_margin) >= height - 1;

    at_edge
        && area * 100 <= width * height * MAX_BADGE_BOX_PERCENT
        && coloured * 100 <= visible * MAX_BADGE_VISIBLE_PERCENT
        && coloured * 100 >= area * MIN_BADGE_FILL_PERCENT
}

pub fn prepare_raster(image: &RgbaImage, size: u16, theme: &ThemeContext) -> Option<RgbaImage> {
    let target = u32::from(size).max(1);
    let mut painted = resize(recolour(&shrink(image, target), theme)?, target);
    sharpen(&mut painted);
    Some(painted)
}

fn shrink(image: &RgbaImage, target: u32) -> Cow<'_, RgbaImage> {
    if image.width.max(image.height) <= target {
        return Cow::Borrowed(image);
    }
    Cow::Owned(resize(image.clone(), target))
}

fn resize(image: RgbaImage, target: u32) -> RgbaImage {
    let longest = image.width.max(image.height).max(1);
    let width = image.width.saturating_mul(target).div_ceil(longest).max(1);
    let height = image.height.saturating_mul(target).div_ceil(longest).max(1);
    let bytes = if width == image.width && height == image.height {
        image.bytes
    } else {
        let mut premultiplied = image.bytes;
        for pixel in premultiplied.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel =
                    u8::try_from((u16::from(*channel) * alpha + 127) / 255).unwrap_or_default();
            }
        }
        let source = image::RgbaImage::from_raw(image.width, image.height, premultiplied)
            .expect("validated raster dimensions");
        let resized = image::imageops::resize(
            &source,
            width,
            height,
            image::imageops::FilterType::CatmullRom,
        );
        let mut bytes = resized.into_raw();
        for pixel in bytes.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            if alpha == 0 {
                pixel[..3].fill(0);
            } else {
                for channel in &mut pixel[..3] {
                    *channel = u8::try_from(
                        ((u32::from(*channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha))
                            .min(255),
                    )
                    .unwrap_or(u8::MAX);
                }
            }
        }
        bytes
    };

    RgbaImage {
        width,
        height,
        bytes,
    }
}

fn sharpen(image: &mut RgbaImage) {
    for pixel in image.bytes.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        let smooth = alpha * alpha * (765 - 2 * alpha) / (255 * 255);
        pixel[3] = u8::try_from((alpha * 3 + smooth) / 4).unwrap_or(u8::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::applet::icons::testing::*;

    #[test]
    fn a_single_ink_pixmap_with_low_contrast_is_recoloured_without_changing_alpha() {
        let image = pixmap(10, |_, y| {
            if (2..8).contains(&y) {
                [245, 245, 245, 180]
            } else {
                [0, 0, 0, 0]
            }
        });
        let before = alphas(&image);

        let out = recolour(&image, &light_panel()).expect("the glyph has insufficient contrast");
        let pixels = out.bytes.as_chunks::<4>().0;

        assert_eq!(alphas(&out), before);
        assert_eq!(&pixels[3 * 10 + 1][..3], &light_panel().ink);
    }

    #[test]
    fn flat_artwork_follows_the_panel_whichever_way_it_already_reads() {
        let white = pixmap(10, |_, y| {
            if (2..8).contains(&y) {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let black = pixmap(10, |_, y| {
            if (2..8).contains(&y) {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });

        for (artwork, theme) in [
            (&white, dark_panel()),
            (&white, light_panel()),
            (&black, dark_panel()),
            (&black, light_panel()),
        ] {
            let out = recolour(artwork, &theme).expect("flat artwork is adapted");
            assert_eq!(&out.bytes.as_chunks::<4>().0[3 * 10 + 1][..3], &theme.ink);
        }
    }

    #[test]
    fn perceptual_lightness_separates_colours_with_the_same_channel_average() {
        assert!(luminance([255, 0, 0, 255]) > luminance([0, 0, 255, 255]));
    }

    #[test]
    fn theme_ink_survives_an_oklab_round_trip() {
        for ink in [[46, 52, 54], [231, 231, 231], [70, 110, 180]] {
            let round_trip = oklab_to_srgb(srgb_to_oklab(ink));
            for channel in 0..3 {
                assert!(round_trip[channel].abs_diff(ink[channel]) <= 1);
            }
        }
    }

    #[test]
    fn tonal_artwork_keeps_its_definition_in_the_theme_ink() {
        let ring = pixmap(12, |x, y| {
            if !(2..10).contains(&x) || !(2..10).contains(&y) {
                [0, 0, 0, 0]
            } else if (4..8).contains(&x) && (4..8).contains(&y) {
                [10, 10, 10, 255]
            } else {
                [250, 250, 250, 255]
            }
        });

        for (theme, base) in [
            (dark_panel(), DARK_PANEL_BASE),
            (light_panel(), LIGHT_PANEL_BASE),
        ] {
            let out = recolour(&ring, &theme).expect("the artwork is painted");
            let pixels = out.bytes.as_chunks::<4>().0;
            let light = pixels[3 * 12 + 3];
            let dark = pixels[6 * 12 + 6];
            let ink = srgb_to_oklab(theme.ink);
            let towards_panel = (srgb_to_oklab(base).lightness - ink.lightness).signum();

            assert!(luminance(light) - luminance(dark) >= 0.3);
            for shade in [light, dark] {
                let shade = srgb_to_oklab([shade[0], shade[1], shade[2]]);
                let travel = (shade.lightness - ink.lightness) * towards_panel;
                assert!(travel >= -ROUND_TRIP_SLACK);
                assert!(travel <= MAX_TINT_SHIFT + ROUND_TRIP_SLACK);
                assert!((shade.a - ink.a).abs() <= ROUND_TRIP_SLACK);
                assert!((shade.b - ink.b).abs() <= ROUND_TRIP_SLACK);
            }
        }
    }

    #[test]
    fn artwork_that_already_reads_on_both_panels_is_still_painted() {
        let neutral = pixmap(10, |_, y| {
            if (2..8).contains(&y) {
                [112, 112, 112, 255]
            } else {
                [0, 0, 0, 0]
            }
        });

        for theme in [dark_panel(), light_panel()] {
            let out = recolour(&neutral, &theme).expect("all artwork is painted");
            assert_eq!(&out.bytes.as_chunks::<4>().0[3 * 10 + 1][..3], &theme.ink);
        }
    }

    #[test]
    fn a_coloured_badge_keeps_its_accent_while_joining_the_theme_palette() {
        let badge = [0, 180, 255, 255];
        let image = pixmap(10, |x, y| match (x, y) {
            (8, 2 | 6) | (7..=9, 3 | 5) | (7 | 9, 4) => badge,
            (_, 2..8) => [255, 255, 255, 255],
            _ => [0, 0, 0, 0],
        });
        let before = alphas(&image);

        let theme = light_panel();
        let out = recolour(&image, &theme).expect("the badge is integrated");
        let pixels = out.bytes.as_chunks::<4>().0;

        assert_eq!(alphas(&out), before);
        assert_eq!(&pixels[3 * 10 + 2][..3], &theme.ink);
        assert_eq!(&pixels[3 * 10 + 8][..3], &badge_colour(badge, theme.ink));
        assert_eq!(
            &pixels[4 * 10 + 8][..3],
            &badge_colour([255, 255, 255, 255], theme.ink)
        );
        assert_eq!(
            &pixels[2 * 10 + 7][..3],
            &badge_colour([255, 255, 255, 255], theme.ink)
        );
        let integrated_badge = pixels[3 * 10 + 8];
        assert!(integrated_badge[2].saturating_sub(integrated_badge[0]) > 80);
    }

    #[test]
    fn a_badge_on_multi_tone_artwork_keeps_the_accent_and_adapts_the_base() {
        let badge = [220, 20, 40, 255];
        let image = pixmap(10, |x, y| match (x, y) {
            (7.., 2..8) => badge,
            (_, 2..5) => [250, 250, 250, 255],
            (_, 5..8) => [20, 20, 20, 255],
            _ => [0, 0, 0, 0],
        });

        let out = recolour(&image, &light_panel()).expect("the tonal base follows the theme");
        let pixels = out.bytes.as_chunks::<4>().0;

        assert_eq!(
            &pixels[3 * 10 + 8][..3],
            &badge_colour(badge, light_panel().ink)
        );
        assert!(luminance(pixels[3 * 10 + 2]) > luminance(pixels[6 * 10 + 2]));
    }

    #[test]
    fn a_high_chroma_badge_is_separated_from_a_slightly_tinted_base() {
        let badge = [240, 71, 71, 255];
        let base = [54, 57, 63, 255];
        let image = pixmap(24, |x, y| match (x, y) {
            (16..23, 2..9) => badge,
            (2..22, 3..21) if (x + y) % 3 == 0 => [245, 245, 245, 255],
            (2..22, 3..21) => base,
            _ => [0, 0, 0, 0],
        });

        let out = recolour(&image, &light_panel()).expect("the edge badge is protected");
        let pixels = out.bytes.as_chunks::<4>().0;

        assert_eq!(
            &pixels[4 * 24 + 18][..3],
            &badge_colour(badge, light_panel().ink)
        );
        assert!(luminance(pixels[11 * 24 + 10]) > luminance(pixels[10 * 24 + 10]));
    }

    #[test]
    fn isolated_highlights_do_not_flatten_the_main_shading() {
        let image = pixmap(20, |x, y| {
            if !(2..18).contains(&x) || !(2..18).contains(&y) {
                [0, 0, 0, 0]
            } else if (x, y) == (2, 2) {
                [255, 255, 255, 255]
            } else if x < 10 {
                [60, 60, 60, 255]
            } else {
                [180, 180, 180, 255]
            }
        });

        let out = recolour(&image, &dark_panel()).expect("the artwork is painted");
        let pixels = out.bytes.as_chunks::<4>().0;

        assert!(luminance(pixels[10 * 20 + 12]) - luminance(pixels[10 * 20 + 6]) >= 0.3);
    }

    #[test]
    fn artwork_carrying_both_dark_and_light_ink_keeps_both_levels() {
        let image = pixmap(12, |x, y| {
            if !(2..10).contains(&x) || !(2..10).contains(&y) {
                [0, 0, 0, 0]
            } else if (3..9).contains(&x) && (3..9).contains(&y) {
                [10, 10, 10, 255]
            } else {
                [245, 245, 245, 255]
            }
        });

        for theme in [dark_panel(), light_panel()] {
            let out = recolour(&image, &theme).expect("the artwork is painted");
            let pixels = out.bytes.as_chunks::<4>().0;
            assert!(luminance(pixels[2 * 12 + 2]) > luminance(pixels[4 * 12 + 4]));
        }
    }

    #[test]
    fn an_antialiased_edge_does_not_pull_the_body_off_the_theme_ink() {
        let image = pixmap(10, |_, y| {
            if (3..7).contains(&y) {
                [250, 250, 250, 255]
            } else if y == 2 || y == 7 {
                [40, 40, 40, 60]
            } else {
                [0, 0, 0, 0]
            }
        });

        let out = recolour(&image, &light_panel()).expect("the glyph does not read here");
        let pixels = out.bytes.as_chunks::<4>().0;

        assert_eq!(&pixels[4 * 10 + 4][..3], &light_panel().ink);
        assert_eq!(pixels[2 * 10 + 4][3], 60);
    }

    #[test]
    fn a_subtly_coloured_pixmap_is_painted() {
        let image = pixmap(10, |_, y| {
            if (2..8).contains(&y) {
                [230, 240, 230, 255]
            } else {
                [0, 0, 0, 0]
            }
        });

        let out = recolour(&image, &light_panel()).expect("coloured artwork is painted");
        assert_eq!(
            &out.bytes.as_chunks::<4>().0[3 * 10 + 1][..3],
            &light_panel().ink
        );
    }

    #[test]
    fn a_multicolour_icon_is_painted_when_its_colours_are_not_a_badge() {
        let image = pixmap(10, |x, y| {
            if !(2..8).contains(&y) {
                [0, 0, 0, 0]
            } else if x < 5 {
                [230, 20, 20, 255]
            } else {
                [20, 80, 230, 255]
            }
        });

        let out = recolour(&image, &light_panel()).expect("multicolour artwork is painted");
        let pixels = out.bytes.as_chunks::<4>().0;
        assert_ne!(&pixels[3 * 10 + 3][..3], &[230, 20, 20]);
        assert_ne!(&pixels[3 * 10 + 7][..3], &[20, 80, 230]);
        assert!(
            (luminance(pixels[3 * 10 + 3]) - luminance(pixels[3 * 10 + 7])).abs() > f32::EPSILON
        );
    }

    #[test]
    fn a_pixmap_with_no_clear_margin_is_painted() {
        let image = pixmap(10, |_, _| [128, 128, 128, 255]);

        let out = recolour(&image, &light_panel()).expect("opaque artwork is painted");
        assert_eq!(&out.bytes.as_chunks::<4>().0[0][..3], &light_panel().ink);
    }

    #[test]
    fn raster_edges_are_gently_sharpened_at_the_final_size() {
        let image = RgbaImage {
            width: 2,
            height: 1,
            bytes: vec![46, 52, 54, 64, 46, 52, 54, 192],
        };

        let mut out = resize(image, 2);
        sharpen(&mut out);
        let pixels = out.bytes.as_chunks::<4>().0;

        assert!(pixels[0][3] < 64);
        assert!(pixels[1][3] > 192);
    }

    #[test]
    fn a_fully_transparent_pixmap_is_left_as_published() {
        let image = pixmap(10, |_, _| [0, 0, 0, 0]);

        assert!(recolour(&image, &light_panel()).is_none());
    }
}
