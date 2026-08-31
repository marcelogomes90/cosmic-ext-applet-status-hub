use std::path::{Path, PathBuf};

use crate::core::icons::RgbaImage;

use super::ThemeContext;

pub fn test_root(suffix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("status-hub-icon-{}-{suffix}", std::process::id()))
}

pub fn svg_at(root: &PathBuf, name: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    let path = root.join(format!("{name}.svg"));
    std::fs::write(&path, body).unwrap();
    path
}

pub fn published_svg(root: &Path, name: &str, body: &str) -> PathBuf {
    let icon_dir = root.join("hicolor/scalable/apps");
    std::fs::create_dir_all(&icon_dir).unwrap();
    let path = icon_dir.join(format!("{name}.svg"));
    std::fs::write(&path, body).unwrap();
    path
}

pub fn png_at(path: &Path, image: &RgbaImage) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::save_buffer_with_format(
        path,
        &image.bytes,
        image.width,
        image.height,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .unwrap();
}

pub fn test_theme(ink: [u8; 3]) -> ThemeContext {
    ThemeContext {
        ink,
        icon_theme: "test".to_owned(),
    }
}

pub fn dark_panel() -> ThemeContext {
    test_theme([231, 231, 231])
}

pub fn light_panel() -> ThemeContext {
    test_theme([46, 52, 54])
}

pub fn pixmap(side: u32, fill: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
    let mut bytes = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            bytes.extend_from_slice(&fill(x, y));
        }
    }
    RgbaImage {
        width: side,
        height: side,
        bytes,
    }
}

pub fn alphas(image: &RgbaImage) -> Vec<u8> {
    image
        .bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[3])
        .collect()
}

pub const ROUND_TRIP_SLACK: f32 = 0.01;

pub const DARK_PANEL_BASE: [u8; 3] = [27, 27, 27];

pub const LIGHT_PANEL_BASE: [u8; 3] = [251, 251, 251];
