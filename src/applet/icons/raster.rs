use std::ffi::OsStr;
use std::path::Path;

use crate::core::icons::RgbaImage;

const MAX_RASTER_BYTES: u64 = 1024 * 1024;

pub fn load(path: &Path, size: u16) -> Option<RgbaImage> {
    if path.extension() == Some(OsStr::new("svg")) {
        return super::svg::render_svg(path, size);
    }
    if !std::fs::metadata(path).is_ok_and(|meta| meta.len() <= MAX_RASTER_BYTES) {
        return None;
    }
    let decoded = image::open(path).ok()?.into_rgba8();
    Some(RgbaImage {
        width: decoded.width(),
        height: decoded.height(),
        bytes: decoded.into_raw(),
    })
}

pub fn may_load_lazily(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || std::fs::File::open(path).is_err() {
        return false;
    }

    if path.extension() == Some(OsStr::new("svg")) {
        metadata.len() > super::svg::MAX_SVG_BYTES
    } else {
        metadata.len() > MAX_RASTER_BYTES
    }
}

pub fn pixels(image: &RgbaImage) -> Option<(usize, usize, &[[u8; 4]])> {
    let width = usize::try_from(image.width).ok()?;
    let height = usize::try_from(image.height).ok()?;
    let (pixels, remainder) = image.bytes.as_chunks::<4>();
    (width > 0
        && height > 0
        && remainder.is_empty()
        && pixels.len() == width.checked_mul(height)?)
    .then_some((width, height, pixels))
}

pub fn prepare(image: RgbaImage, size: u16) -> Option<RgbaImage> {
    pixels(&image)?;
    let target = (u32::from(size).max(1) * 2).min(image.width.max(image.height));
    Some(resize(image, target))
}

pub fn straighten(channel: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        return 0;
    }
    let alpha = u32::from(alpha);
    u8::try_from(((u32::from(channel) * 255 + alpha / 2) / alpha).min(255)).unwrap_or(u8::MAX)
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
            let alpha = pixel[3];
            for channel in &mut pixel[..3] {
                *channel = straighten(*channel, alpha);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::applet::icons::testing::*;

    #[test]
    fn small_sources_and_transparent_pixels_are_not_modified() {
        let image = RgbaImage {
            width: 2,
            height: 1,
            bytes: vec![200, 100, 50, 128, 12, 34, 56, 0],
        };
        assert_eq!(prepare(image.clone(), 24), Some(image));
    }

    #[test]
    fn every_source_uses_the_same_hidpi_limit_and_aspect_ratio() {
        for size in [18, 22, 24] {
            for colour in [[255; 4], [220, 30, 90, 255]] {
                let image = RgbaImage {
                    width: 120,
                    height: 60,
                    bytes: colour.repeat(120 * 60),
                };
                let result = prepare(image, size).unwrap();
                assert_eq!(
                    (result.width, result.height),
                    (u32::from(size) * 2, u32::from(size))
                );
            }
        }
    }

    #[test]
    fn transparent_rgb_does_not_leak_into_filtered_edges() {
        let image = pixmap(64, |x, y| {
            if (16..48).contains(&x) && (16..48).contains(&y) {
                [255, 0, 0, 255]
            } else {
                [0, 255, 0, 0]
            }
        });
        let resized = prepare(image, 18).unwrap();
        for pixel in resized.bytes.as_chunks::<4>().0 {
            assert_eq!(pixel[1], 0);
            assert_eq!(pixel[2], 0);
            if pixel[3] > 0 {
                assert_eq!(pixel[0], 255);
            }
        }
    }

    #[test]
    fn zero_size_is_bounded_and_invalid_images_are_rejected() {
        assert_eq!(prepare(pixmap(4, |_, _| [255; 4]), 0).unwrap().width, 2);
        for image in [
            RgbaImage {
                width: 0,
                height: 1,
                bytes: vec![],
            },
            RgbaImage {
                width: 1,
                height: 1,
                bytes: vec![0; 3],
            },
            RgbaImage {
                width: u32::MAX,
                height: u32::MAX,
                bytes: vec![],
            },
        ] {
            assert!(prepare(image, 24).is_none());
        }
        assert!(prepare(pixmap(4, |_, _| [0; 4]), 24).is_some());
    }

    #[test]
    fn straightening_handles_zero_partial_and_full_alpha() {
        assert_eq!(straighten(100, 0), 0);
        assert_eq!(straighten(64, 128), 128);
        assert_eq!(straighten(200, 255), 200);
        assert_eq!(straighten(255, 16), 255);
    }

    #[test]
    fn missing_and_invalid_small_files_are_not_deferred_to_the_renderer() {
        let root = crate::applet::icons::testing::test_root("invalid-lazy-file");
        std::fs::create_dir_all(&root).unwrap();
        let missing = root.join("missing.png");
        let invalid = root.join("invalid.png");
        std::fs::write(&invalid, b"not a png").unwrap();

        assert!(!may_load_lazily(&missing));
        assert!(load(&invalid, 24).is_none());
        assert!(!may_load_lazily(&invalid));

        std::fs::remove_dir_all(root).unwrap();
    }
}
