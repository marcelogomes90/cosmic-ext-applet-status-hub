use std::sync::Arc;

use crate::core::model::{IconSource, ItemStatus, Pixmap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IconKind {
    Primary,
    Overlay,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IconOptions {
    pub name: Option<String>,
    pub theme_path: Option<String>,
    pub path: Option<String>,
    pub pixels: Option<Arc<RgbaImage>>,
}

impl IconOptions {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.path.is_none() && self.pixels.is_none()
    }
}

pub fn resolve(
    source: &IconSource,
    status: ItemStatus,
    kind: IconKind,
    target_px: u32,
) -> IconOptions {
    let (name, pixmaps) = match kind {
        IconKind::Overlay => (
            source.overlay_icon_name.as_str(),
            source.overlay_icon_pixmap.as_slice(),
        ),
        IconKind::Primary if status == ItemStatus::NeedsAttention => {
            if source.attention_icon_name.trim().is_empty()
                && !source.attention_icon_pixmap.iter().any(Pixmap::is_valid)
            {
                (source.icon_name.as_str(), source.icon_pixmap.as_slice())
            } else {
                (
                    source.attention_icon_name.as_str(),
                    source.attention_icon_pixmap.as_slice(),
                )
            }
        }
        IconKind::Primary => (source.icon_name.as_str(), source.icon_pixmap.as_slice()),
    };

    let name = name.trim();
    let (name, path) = if name.is_empty() {
        (None, None)
    } else if name.starts_with('/') {
        (None, Some(name.to_owned()))
    } else {
        (Some(name.to_owned()), None)
    };

    IconOptions {
        name,
        theme_path: source.theme_path.clone(),
        path,
        pixels: best_frame(pixmaps, target_px).map(|frame| Arc::new(decode(frame))),
    }
}

fn best_frame(pixmaps: &[Pixmap], target_px: u32) -> Option<&Pixmap> {
    let usable = || pixmaps.iter().filter(|frame| frame.is_valid());

    let target = i32::try_from(target_px).unwrap_or(i32::MAX);
    usable()
        .filter(|frame| frame.width >= target)
        .min_by_key(|frame| frame.width)
        .or_else(|| usable().max_by_key(|frame| frame.width))
}

fn decode(frame: &Pixmap) -> RgbaImage {
    let mut bytes = frame.bytes.clone();
    for pixel in bytes.as_chunks_mut::<4>().0 {
        pixel.rotate_left(1);
    }

    RgbaImage {
        width: frame.width.unsigned_abs(),
        height: frame.height.unsigned_abs(),
        bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixmap(size: i32) -> Pixmap {
        Pixmap {
            width: size,
            height: size,
            bytes: vec![0; usize::try_from(size * size * 4).unwrap()],
        }
    }

    fn named(name: &str) -> IconSource {
        IconSource {
            icon_name: name.to_owned(),
            ..IconSource::default()
        }
    }

    fn primary(source: &IconSource) -> IconOptions {
        resolve(source, ItemStatus::Active, IconKind::Primary, 24)
    }

    #[test]
    fn a_name_and_pixels_are_both_offered_when_both_were_published() {
        let source = IconSource {
            icon_name: "steam".to_owned(),
            icon_pixmap: vec![pixmap(64)],
            ..IconSource::default()
        };
        let options = primary(&source);
        assert_eq!(options.name.as_deref(), Some("steam"));
        assert!(options.pixels.is_some());
    }

    #[test]
    fn the_theme_path_travels_with_the_name() {
        let source = IconSource {
            theme_path: Some("/opt/app/icons".to_owned()),
            ..named("custom")
        };
        assert_eq!(
            primary(&source).theme_path.as_deref(),
            Some("/opt/app/icons")
        );
    }

    #[test]
    fn an_absolute_icon_name_is_treated_as_a_file() {
        let options = primary(&named("/usr/share/pixmaps/thing.png"));
        assert_eq!(
            options.path.as_deref(),
            Some("/usr/share/pixmaps/thing.png")
        );
        assert!(options.name.is_none());
    }

    #[test]
    fn attention_status_uses_the_attention_icon() {
        let source = IconSource {
            icon_name: "calm".to_owned(),
            attention_icon_name: "urgent".to_owned(),
            ..IconSource::default()
        };
        let options = resolve(&source, ItemStatus::NeedsAttention, IconKind::Primary, 24);
        assert_eq!(options.name.as_deref(), Some("urgent"));
    }

    #[test]
    fn attention_status_without_an_attention_icon_keeps_the_normal_one() {
        let options = resolve(
            &named("calm"),
            ItemStatus::NeedsAttention,
            IconKind::Primary,
            24,
        );
        assert_eq!(options.name.as_deref(), Some("calm"));
    }

    #[test]
    fn invalid_attention_pixmaps_do_not_hide_the_normal_icon() {
        let source = IconSource {
            icon_name: "calm".to_owned(),
            attention_icon_pixmap: vec![Pixmap {
                width: 64,
                height: 64,
                bytes: Vec::new(),
            }],
            ..IconSource::default()
        };
        let options = resolve(&source, ItemStatus::NeedsAttention, IconKind::Primary, 24);
        assert_eq!(options.name.as_deref(), Some("calm"));
    }

    #[test]
    fn nothing_published_is_reported_as_empty() {
        assert!(primary(&IconSource::default()).is_empty());
        assert!(resolve(&named("app"), ItemStatus::Active, IconKind::Overlay, 24).is_empty());
    }

    #[test]
    fn the_smallest_frame_at_or_above_the_target_wins() {
        let frames = [pixmap(16), pixmap(22), pixmap(32), pixmap(64)];
        assert_eq!(best_frame(&frames, 22).unwrap().width, 22);
        assert_eq!(best_frame(&frames, 24).unwrap().width, 32);
        assert_eq!(best_frame(&frames, 48).unwrap().width, 64);
    }

    #[test]
    fn an_oversized_request_settles_for_the_largest_frame() {
        let frames = [pixmap(16), pixmap(22)];
        assert_eq!(best_frame(&frames, 256).unwrap().width, 22);
    }

    #[test]
    fn malformed_frames_are_skipped_rather_than_trusted() {
        let frames = [
            Pixmap {
                width: 64,
                height: 64,
                bytes: vec![0; 16],
            },
            pixmap(22),
        ];
        assert_eq!(best_frame(&frames, 24).unwrap().width, 22);

        let only_broken = [Pixmap {
            width: 64,
            height: 64,
            bytes: Vec::new(),
        }];
        assert!(best_frame(&only_broken, 24).is_none());
    }

    #[test]
    fn decoding_rotates_argb_into_rgba() {
        let frame = Pixmap {
            width: 1,
            height: 2,
            bytes: vec![0x11, 0x22, 0x33, 0x44, 0xAA, 0xBB, 0xCC, 0xDD],
        };
        let decoded = decode(&frame);
        assert_eq!(decoded.width, 1);
        assert_eq!(decoded.height, 2);
        assert_eq!(
            decoded.bytes,
            vec![0x22, 0x33, 0x44, 0x11, 0xBB, 0xCC, 0xDD, 0xAA,]
        );
    }

    #[test]
    fn a_pixmap_only_item_still_gets_an_icon() {
        let source = IconSource {
            icon_pixmap: vec![pixmap(32)],
            ..IconSource::default()
        };
        let image = primary(&source).pixels.expect("expected decoded pixels");
        assert_eq!((image.width, image.height), (32, 32));
        assert_eq!(image.bytes.len(), 32 * 32 * 4);
    }
}
