use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use cosmic::widget::icon::{self, Named};

use crate::core::icons::{IconKind, IconOptions, RgbaImage, resolve};
use crate::core::model::{Generation, ItemAddress, TraySnapshot};

mod paint;
mod raster;
mod svg;
#[cfg(test)]
mod testing;

use self::paint::recolour;

const FALLBACKS: [&str; 2] = ["application-default", "application-x-executable"];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    address: ItemAddress,
    generation: Generation,
    kind: IconKind,
    size: u16,
}

#[derive(Debug)]
struct Entry {
    handle: Option<icon::Handle>,
    fallback: bool,
}

#[derive(Clone, Debug)]
pub struct TrayIcon {
    pub primary: icon::Handle,
    pub overlay: Option<icon::Handle>,
}

pub const fn overlay_size(size: u16) -> u16 {
    size.div_ceil(2)
}

#[derive(Debug, Default)]
pub struct IconCache {
    entries: HashMap<Key, Entry>,
    theme: ThemeContext,
}

impl IconCache {
    pub fn refresh(
        &mut self,
        snapshot: &TraySnapshot,
        size: u16,
        retry_fallbacks: bool,
        colour_icons: bool,
    ) -> bool {
        self.refresh_with_theme(snapshot, size, retry_fallbacks, theme_context(colour_icons))
    }

    fn refresh_with_theme(
        &mut self,
        snapshot: &TraySnapshot,
        size: u16,
        retry_fallbacks: bool,
        theme: ThemeContext,
    ) -> bool {
        if theme != self.theme {
            self.entries.clear();
            self.theme = theme;
        }
        let theme = &self.theme;

        let overlay_theme = ThemeContext {
            colour_icons: false,
            ..theme.clone()
        };
        let mut next = HashMap::with_capacity(snapshot.items.len() * 2);
        let mut unresolved_primary = false;

        for item in &snapshot.items {
            for kind in [IconKind::Primary, IconKind::Overlay] {
                let key = Key {
                    address: item.address.clone(),
                    generation: item.generation,
                    kind,
                    size,
                };
                let entry = match self.entries.remove(&key) {
                    Some(entry) if !entry.fallback || !retry_fallbacks => entry,
                    _ => {
                        let draw_size = if kind == IconKind::Overlay {
                            overlay_size(size)
                        } else {
                            size
                        };
                        let options = resolve(
                            &item.icon,
                            item.status,
                            kind,
                            u32::from(draw_size).max(1) * 2,
                        );
                        let built = if kind == IconKind::Overlay {
                            build_artwork(&options, draw_size, &overlay_theme, kind)
                        } else {
                            Some(build(&options, &item.id, draw_size, theme))
                        };
                        if let Some(built) = built {
                            tracing::info!(
                                item = %item.id,
                                ?kind,
                                size,
                                name = options.name.as_deref().unwrap_or("-"),
                                theme_path = options.theme_path.as_deref().unwrap_or("-"),
                                pixmap = options.pixels.is_some(),
                                source = %built.source,
                                symbolic = built.handle.symbolic,
                                paint = built.paint,
                                rasterized = matches!(&built.handle.data, icon::Data::Image(_)),
                                "icon resolved"
                            );
                            Entry {
                                handle: Some(built.handle),
                                fallback: built.fallback,
                            }
                        } else {
                            Entry {
                                handle: None,
                                fallback: false,
                            }
                        }
                    }
                };
                if entry.fallback {
                    unresolved_primary = true;
                }
                next.insert(key, entry);
            }
        }

        self.entries = next;
        unresolved_primary
    }

    pub fn get(
        &self,
        address: &ItemAddress,
        generation: Generation,
        kind: IconKind,
        size: u16,
    ) -> Option<&icon::Handle> {
        self.entries
            .get(&Key {
                address: address.clone(),
                generation,
                kind,
                size,
            })
            .and_then(|entry| entry.handle.as_ref())
    }

    pub fn item(
        &self,
        address: &ItemAddress,
        generation: Generation,
        size: u16,
    ) -> Option<TrayIcon> {
        Some(TrayIcon {
            primary: self
                .get(address, generation, IconKind::Primary, size)?
                .clone(),
            overlay: self
                .get(address, generation, IconKind::Overlay, size)
                .cloned(),
        })
    }
}

struct Built {
    handle: icon::Handle,
    source: String,
    fallback: bool,
    paint: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    Published,
    Payload,
}

fn build(options: &IconOptions, item_id: &str, size: u16, theme: &ThemeContext) -> Built {
    build_artwork(options, size, theme, IconKind::Primary)
        .or_else(|| fallback_to_id(item_id, options.name.as_deref(), size, theme))
        .unwrap_or_else(|| fallback(size, theme))
}

fn fallback_to_id(
    item_id: &str,
    published_name: Option<&str>,
    size: u16,
    theme: &ThemeContext,
) -> Option<Built> {
    let item_id = item_id.trim();
    if item_id.is_empty()
        || published_name == Some(item_id)
        || item_id.contains(['/', '\\'])
        || matches!(item_id, "." | "..")
    {
        return None;
    }

    let path = lookup(item_id, size)?;
    let source = format!("id {item_id} -> {}", path.display());
    let mut built = from_file(path, item_id, source, size, theme, IconKind::Primary)?;
    built.fallback = true;
    Some(built)
}

fn build_artwork(
    options: &IconOptions,
    size: u16,
    theme: &ThemeContext,
    kind: IconKind,
) -> Option<Built> {
    let file = |path: PathBuf, name: &str, source| from_file(path, name, source, size, theme, kind);
    if let Some(name) = &options.name {
        if let Some(path) = lookup(name, size) {
            let source = format!("name {name} -> {}", path.display());
            if let Some(built) = file(path, name, source) {
                return Some(built);
            }
        }

        if let Some((path, origin)) = options
            .theme_path
            .as_deref()
            .and_then(|root| lookup_published(name, root, size))
        {
            let source = format!("{} name {name} -> {}", origin.label(), path.display());
            if let Some(built) = file(path, name, source) {
                return Some(built);
            }
        }
    }

    if let Some(published) = &options.path
        && let Some((path, origin)) = resolve_path(published)
    {
        let source = format!("{} path {}", origin.label(), path.display());
        if let Some(built) = file(path, "", source) {
            return Some(built);
        }
    }

    if let Some(published) = &options.pixels {
        let explicit = options
            .name
            .as_deref()
            .is_some_and(|name| name.ends_with("-symbolic"));
        let (handle, paint) = prepared_handle(published.as_ref().clone(), size, theme, explicit)?;
        return Some(Built {
            handle,
            source: format!("pixmap {}x{}", published.width, published.height),
            fallback: false,
            paint,
        });
    }

    None
}

fn fallback(size: u16, theme: &ThemeContext) -> Built {
    for fallback in FALLBACKS {
        if let Some(path) = lookup(fallback, size) {
            let source = format!("GENERIC {fallback} -> {}", path.display());
            let handle = raster::load(&path, size)
                .and_then(|image| prepared_handle(image, size, theme, true))
                .map_or_else(
                    || {
                        let mut handle = icon::from_path(path);
                        handle.symbolic = true;
                        handle
                    },
                    |(handle, _)| handle,
                );
            let paint = "symbolic-fallback";
            return Built {
                handle,
                source,
                fallback: true,
                paint,
            };
        }
    }

    Built {
        handle: icon::from_name(FALLBACKS[0])
            .size(size)
            .symbolic(true)
            .handle(),
        source: "GENERIC unresolved".to_owned(),
        fallback: true,
        paint: "symbolic-fallback",
    }
}

fn from_file(
    path: PathBuf,
    name: &str,
    source: String,
    size: u16,
    theme: &ThemeContext,
    kind: IconKind,
) -> Option<Built> {
    let explicit = name.ends_with("-symbolic")
        || path
            .file_stem()
            .and_then(OsStr::to_str)
            .is_some_and(|stem| stem.ends_with("-symbolic"));
    let (handle, paint) = if let Some(prepared) =
        raster::load(&path, size).and_then(|image| prepared_handle(image, size, theme, explicit))
    {
        prepared
    } else if kind == IconKind::Overlay || !raster::may_load_lazily(&path) {
        return None;
    } else {
        let mut handle = icon::from_path(path);
        handle.symbolic = explicit;
        (
            handle,
            if explicit {
                "symbolic-explicit"
            } else {
                "original-unprocessed"
            },
        )
    };
    Some(Built {
        handle,
        source,
        fallback: false,
        paint,
    })
}

fn prepared_handle(
    mut image: RgbaImage,
    size: u16,
    theme: &ThemeContext,
    explicit: bool,
) -> Option<(icon::Handle, &'static str)> {
    let decision = recolour(&mut image, theme, explicit);
    let image = raster::prepare(image, size)?;
    Some((
        icon::from_raster_pixels(image.width, image.height, image.bytes),
        decision.label(),
    ))
}

impl Origin {
    fn label(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Payload => "payload",
        }
    }
}

fn resolve_path(published: &str) -> Option<(PathBuf, Origin)> {
    let path = PathBuf::from(published);
    if path.exists() {
        return Some((path, Origin::Published));
    }
    crate::flatpak::payload_file(published).map(|path| (path, Origin::Payload))
}

fn lookup_published(name: &str, root: &str, size: u16) -> Option<(PathBuf, Origin)> {
    if root.is_empty() {
        return None;
    }

    if Path::new(root).is_dir() {
        return search(name, Path::new(root), size).map(|path| (path, Origin::Published));
    }

    crate::flatpak::payload_roots(root)
        .iter()
        .find_map(|root| search(name, root, size))
        .map(|path| (path, Origin::Payload))
}

fn search(name: &str, root: &Path, size: u16) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let roots = [root.clone()];
    let find = |prefer_svg| {
        let mut lookup = cosmic_freedesktop_icons::lookup(name)
            .with_extra_paths(&roots)
            .with_size(size);
        if prefer_svg {
            lookup = lookup.force_svg();
        }
        lookup.find().filter(|path| path.starts_with(&root))
    };
    find(true).or_else(|| find(false))
}

fn lookup(name: &str, size: u16) -> Option<PathBuf> {
    named(name)
        .size(size)
        .prefer_svg(true)
        .path()
        .or_else(|| named(name).prefer_svg(false).path())
}

fn named(name: &str) -> Named {
    icon::from_name(name.to_owned())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ThemeContext {
    ink: [u8; 3],
    background: [u8; 3],
    icon_theme: String,
    colour_icons: bool,
}

fn theme_context(colour_icons: bool) -> ThemeContext {
    let theme = cosmic::theme::active();
    let container = theme.cosmic().background(theme.transparent);
    let ink = container.on.into_format::<u8, u8>();
    let background = container.base.into_format::<u8, u8>();
    ThemeContext {
        ink: [ink.red, ink.green, ink.blue],
        background: [background.red, background.green, background.blue],
        icon_theme: cosmic::icon_theme::default(),
        colour_icons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::applet::icons::testing::*;
    use crate::core::model::Pixmap;
    use crate::core::testing::item;

    #[test]
    fn png_and_pixmap_preparation_is_independent_of_the_paint_decision() {
        let root = test_root("unified-raster");
        for (name, colour) in [
            ("neutral", [200, 200, 200, 255]),
            ("coloured", [20, 100, 200, 255]),
        ] {
            let image = RgbaImage {
                width: 120,
                height: 60,
                bytes: colour.repeat(120 * 60),
            };
            let path = root.join(format!("{name}.png"));
            png_at(&path, &image);
            for size in [18, 22, 24] {
                for theme in [light_panel(), dark_panel(), original_icons()] {
                    let from_path = test_file(path.clone(), name, size, &theme);
                    let from_pixels = prepared_handle(image.clone(), size, &theme, false).unwrap();
                    assert_eq!(from_path.1, from_pixels.1);
                    assert_eq!(raster_pixels(&from_path.0), raster_pixels(&from_pixels.0));
                    let (width, height, pixels) = raster_pixels(&from_path.0);
                    assert_eq!((width, height), (u32::from(size) * 2, u32::from(size)));
                    let expected = if name == "neutral" && theme.colour_icons {
                        theme.ink
                    } else {
                        [colour[0], colour[1], colour[2]]
                    };
                    assert_eq!(&pixels[..3], &expected);
                }
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn svg_preparation_keeps_resolution_and_aspect_when_paint_is_disabled_or_rejected() {
        let root = test_root("unified-svg");
        for (name, fill) in [("neutral", "#cccccc"), ("coloured", "#1464c8")] {
            let path = svg_at(
                &root,
                name,
                &format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 12\"><rect width=\"24\" height=\"12\" fill=\"{fill}\"/></svg>"
                ),
            );
            for size in [18, 22, 24] {
                for theme in [light_panel(), dark_panel(), original_icons()] {
                    let (handle, policy) = test_file(path.clone(), name, size, &theme);
                    let (width, height, _) = raster_pixels(&handle);
                    assert_eq!((width, height), (u32::from(size) * 2, u32::from(size)));
                    let expected = if !theme.colour_icons {
                        "original-disabled"
                    } else if name == "neutral" {
                        "monotone"
                    } else {
                        "original-coloured"
                    };
                    assert_eq!(policy, expected);
                }
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn original_overlays_are_resized_independently_of_the_primary_preference() {
        let mut tray_item = item("large-overlay", 1);
        let source = std::sync::Arc::make_mut(&mut tray_item.icon);
        source.icon_pixmap = vec![frame(96, [200, 200, 200, 255])];
        source.overlay_icon_pixmap = vec![frame(64, [224, 30, 90, 255])];
        let snapshot = TraySnapshot {
            items: vec![tray_item],
            ..TraySnapshot::default()
        };
        let mut cache = IconCache::default();
        for theme in [light_panel(), original_icons()] {
            cache.refresh_with_theme(&snapshot, 24, false, theme);
            let item = &snapshot.items[0];
            let icon = cache.item(&item.address, item.generation, 24).unwrap();
            let (width, height, pixels) = raster_pixels(icon.overlay.as_ref().unwrap());
            assert_eq!((width, height), (24, 24));
            assert_eq!(&pixels[..4], &[224, 30, 90, 255]);
        }
    }

    #[test]
    fn cache_size_and_primary_generation_changes_rebuild_prepared_images() {
        let mut tray_item = item("resizing-primary", 1);
        std::sync::Arc::make_mut(&mut tray_item.icon).icon_pixmap =
            vec![frame(96, [200, 200, 200, 255])];
        let mut snapshot = TraySnapshot {
            items: vec![tray_item],
            ..TraySnapshot::default()
        };
        let mut cache = IconCache::default();
        for size in [18, 24] {
            cache.refresh_with_theme(&snapshot, size, false, light_panel());
            let item = &snapshot.items[0];
            let icon = cache.item(&item.address, item.generation, size).unwrap();
            assert_eq!(raster_pixels(&icon.primary).0, u32::from(size) * 2);
        }
        let item = &mut snapshot.items[0];
        item.generation.0 += 1;
        std::sync::Arc::make_mut(&mut item.icon).icon_pixmap = vec![frame(96, [220, 20, 20, 255])];
        cache.refresh_with_theme(&snapshot, 24, false, light_panel());
        let item = &snapshot.items[0];
        let icon = cache.item(&item.address, item.generation, 24).unwrap();
        assert_eq!(&raster_pixels(&icon.primary).2[..4], &[220, 20, 20, 255]);
    }
    fn test_file(
        path: PathBuf,
        name: &str,
        size: u16,
        theme: &ThemeContext,
    ) -> (icon::Handle, &'static str) {
        let built = from_file(path, name, String::new(), size, theme, IconKind::Primary).unwrap();
        (built.handle, built.paint)
    }

    fn raster_pixels(handle: &icon::Handle) -> (u32, u32, &[u8]) {
        let icon::Data::Image(cosmic::iced::widget::image::Handle::Rgba {
            width,
            height,
            pixels,
            ..
        }) = &handle.data
        else {
            panic!("expected decoded raster");
        };
        (*width, *height, pixels.as_ref())
    }

    fn frame(size: i32, rgba: [u8; 4]) -> Pixmap {
        let [r, g, b, a] = rgba;
        Pixmap {
            width: size,
            height: size,
            bytes: [a, r, g, b].repeat(usize::try_from(size * size).unwrap()),
        }
    }

    #[test]
    fn the_cache_selects_hidpi_frames_and_preserves_overlay_colours() {
        let mut tray_item = item("hidpi", 1);
        let source = std::sync::Arc::make_mut(&mut tray_item.icon);
        source.icon_pixmap = vec![
            frame(24, [240; 4]),
            frame(48, [240; 4]),
            frame(64, [240; 4]),
        ];
        source.overlay_icon_pixmap =
            vec![frame(12, [224, 30, 90, 255]), frame(24, [224, 30, 90, 255])];
        let address = tray_item.address.clone();
        let generation = tray_item.generation;
        let snapshot = TraySnapshot {
            items: vec![tray_item],
            ..TraySnapshot::default()
        };
        let mut cache = IconCache::default();
        for theme in [dark_panel(), light_panel(), original_icons()] {
            assert!(!cache.refresh_with_theme(&snapshot, 24, false, theme));
            let icon = cache.item(&address, generation, 24).unwrap();
            let (width, height, _) = raster_pixels(&icon.primary);
            assert_eq!((width, height), (48, 48));
            let (width, height, pixels) = raster_pixels(icon.overlay.as_ref().unwrap());
            assert_eq!((width, height), (24, 24));
            assert_eq!(pixels, [224, 30, 90, 255].repeat(24 * 24));
        }
    }

    #[test]
    fn overlays_appear_change_and_disappear_with_item_generations() {
        let mut tray_item = item("overlay", 1);
        std::sync::Arc::make_mut(&mut tray_item.icon).icon_pixmap = vec![frame(24, [255; 4])];
        let address = tray_item.address.clone();
        let mut snapshot = TraySnapshot {
            items: vec![tray_item],
            ..TraySnapshot::default()
        };
        let mut cache = IconCache::default();
        for colour in [None, Some([255, 0, 0, 255]), Some([0, 180, 255, 255]), None] {
            let tray_item = &mut snapshot.items[0];
            tray_item.generation.0 += 1;
            let generation = tray_item.generation;
            std::sync::Arc::make_mut(&mut tray_item.icon).overlay_icon_pixmap =
                colour.map(|c| vec![frame(12, c)]).unwrap_or_default();
            assert!(!cache.refresh_with_theme(&snapshot, 24, false, dark_panel()));
            let icon = cache.item(&address, generation, 24).unwrap();
            if let Some(colour) = colour {
                assert_eq!(
                    &raster_pixels(icon.overlay.as_ref().unwrap()).2[..4],
                    &colour
                );
            } else {
                assert!(icon.overlay.is_none());
            }
            assert_eq!(cache.entries.len(), 2);
        }
    }

    #[test]
    fn invalid_overlay_files_use_the_pixmap_or_remain_absent() {
        let root = test_root("invalid-overlay");
        let broken = svg_at(&root, "broken", "not an svg");
        let mut options = IconOptions {
            path: Some(broken.to_string_lossy().into_owned()),
            ..IconOptions::default()
        };
        assert!(build_artwork(&options, 12, &original_icons(), IconKind::Overlay).is_none());
        options.pixels = Some(std::sync::Arc::new(pixmap(12, |_, _| [224, 30, 90, 255])));
        let built = build_artwork(&options, 12, &original_icons(), IconKind::Overlay).unwrap();
        assert_eq!(&raster_pixels(&built.handle).2[..4], &[224, 30, 90, 255]);
        assert!(!built.fallback);
        assert!(
            build_artwork(
                &IconOptions::default(),
                12,
                &original_icons(),
                IconKind::Overlay
            )
            .is_none()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_explicit_symbolic_overlays_follow_the_theme() {
        let root = test_root("symbolic-overlay");
        for (name, symbolic) in [("plain", false), ("plain-symbolic", true)] {
            let path = svg_at(
                &root,
                name,
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"12\" height=\"12\"><rect width=\"12\" height=\"12\" fill=\"#444\"/></svg>",
            );
            let options = IconOptions {
                path: Some(path.to_string_lossy().into_owned()),
                ..IconOptions::default()
            };
            let built = build_artwork(&options, 12, &original_icons(), IconKind::Overlay).unwrap();
            assert!(!built.handle.symbolic);
            assert_eq!(
                built.paint,
                if symbolic {
                    "symbolic-explicit"
                } else {
                    "original-disabled"
                }
            );
            let expected = if symbolic {
                original_icons().ink
            } else {
                [68; 3]
            };
            assert_eq!(&raster_pixels(&built.handle).2[..3], &expected);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_published_path_is_found_without_waiting_for_the_global_cache() {
        let root = test_root("published-path");
        let icon_dir = root.join("hicolor/scalable/apps");
        std::fs::create_dir_all(&icon_dir).unwrap();
        let name = format!("status-hub-late-icon-{}", std::process::id());

        assert!(lookup(&name, 24).is_none());
        let icon_path = icon_dir.join(format!("{name}.svg"));
        std::fs::write(&icon_path, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();

        assert_eq!(
            lookup_published(&name, root.to_str().unwrap(), 24),
            Some((icon_path.canonicalize().unwrap(), Origin::Published))
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_fallback_is_replaced_when_the_published_file_appears() {
        let root = test_root("fallback-retry");
        let icon_dir = root.join("hicolor/scalable/apps");
        std::fs::create_dir_all(&icon_dir).unwrap();
        let name = format!("status-hub-retried-icon-{}", std::process::id());

        let mut tray_item = item("late", 1);
        let source = std::sync::Arc::make_mut(&mut tray_item.icon);
        source.icon_name.clone_from(&name);
        source.theme_path = Some(root.to_string_lossy().into_owned());
        let snapshot = TraySnapshot {
            items: vec![tray_item],
            ..TraySnapshot::default()
        };
        let mut cache = IconCache::default();

        assert!(cache.refresh(&snapshot, 24, false, true));
        let icon_path = icon_dir.join(format!("{name}.svg"));
        std::fs::write(icon_path, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
        assert!(!cache.refresh(&snapshot, 24, true, true));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn colour_theme_icon_theme_or_paint_mode_changes_rebuild_cached_pixmaps() {
        use std::hash::{Hash, Hasher};

        let mut tray_item = item("theme-cache", 1);
        let source = std::sync::Arc::make_mut(&mut tray_item.icon);
        source.icon_name.clear();
        source.icon_pixmap = vec![Pixmap {
            width: 2,
            height: 2,
            bytes: vec![0; 2 * 2 * 4],
        }];
        let address = tray_item.address.clone();
        let generation = tray_item.generation;
        let snapshot = TraySnapshot {
            items: vec![tray_item],
            ..TraySnapshot::default()
        };
        let mut cache = IconCache::default();
        let hash = |cache: &IconCache| {
            let mut state = std::collections::hash_map::DefaultHasher::new();
            cache
                .get(&address, generation, IconKind::Primary, 24)
                .unwrap()
                .hash(&mut state);
            state.finish()
        };

        let first_theme = test_theme([0; 3]);
        cache.refresh_with_theme(&snapshot, 24, false, first_theme.clone());
        let first = hash(&cache);
        cache.refresh_with_theme(&snapshot, 24, false, first_theme.clone());
        assert_eq!(hash(&cache), first, "an unchanged theme reuses the handle");

        let mut other_icons = first_theme.clone();
        other_icons.icon_theme = "other".to_owned();
        cache.refresh_with_theme(&snapshot, 24, false, other_icons);
        let second = hash(&cache);
        assert_ne!(second, first, "an icon theme change rebuilds the handle");

        cache.refresh_with_theme(&snapshot, 24, false, test_theme([255; 3]));
        let third = hash(&cache);
        assert_ne!(third, second, "a colour theme change rebuilds the handle");

        cache.refresh_with_theme(&snapshot, 24, false, original_icons());
        assert_ne!(
            hash(&cache),
            third,
            "the original colour mode rebuilds the handle"
        );

        let original = hash(&cache);
        let mut other_background = original_icons();
        other_background.background = [200; 3];
        cache.refresh_with_theme(&snapshot, 24, false, other_background);
        assert_ne!(
            hash(&cache),
            original,
            "a background change rebuilds the handle"
        );
    }

    #[test]
    fn the_global_theme_including_its_name_fallback_wins_over_the_app_and_pixmap() {
        let Some(themed) = FALLBACKS.iter().copied().find(|n| lookup(n, 24).is_some()) else {
            return;
        };
        let root = test_root("global-priority");
        let name = format!("{themed}-status-hub-specific");
        let app_path = published_svg(&root, &name, "<svg/>");
        let options = IconOptions {
            name: Some(name.clone()),
            theme_path: Some(root.to_string_lossy().into_owned()),
            pixels: Some(std::sync::Arc::new(pixmap(24, |_, _| [0, 0, 0, 0]))),
            ..IconOptions::default()
        };

        let built = build(&options, "", 24, &test_theme([0; 3]));

        assert!(built.source.starts_with("name "), "{}", built.source);
        assert!(
            !built
                .source
                .contains(&app_path.to_string_lossy().into_owned())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_apps_theme_path_wins_over_a_published_pixmap() {
        let root = test_root("app-priority");
        let name = format!("status-hub-app-priority-{}", std::process::id());
        let app_path = published_svg(&root, &name, "<svg/>");
        let options = IconOptions {
            name: Some(name.clone()),
            theme_path: Some(root.to_string_lossy().into_owned()),
            pixels: Some(std::sync::Arc::new(pixmap(24, |_, _| [0, 0, 0, 0]))),
            ..IconOptions::default()
        };

        let built = build(&options, "", 24, &test_theme([0; 3]));

        assert_eq!(
            built.source,
            format!(
                "published name {name} -> {}",
                app_path.canonicalize().unwrap().display()
            )
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_absolute_path_wins_over_a_published_pixmap() {
        let root = test_root("absolute-priority");
        let path = svg_at(&root, "absolute", "<svg/>");
        let options = IconOptions {
            path: Some(path.to_string_lossy().into_owned()),
            pixels: Some(std::sync::Arc::new(pixmap(24, |_, _| [0, 0, 0, 0]))),
            ..IconOptions::default()
        };

        let built = build(&options, "", 24, &test_theme([0; 3]));

        assert_eq!(built.source, format!("published path {}", path.display()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_pixmap_wins_over_the_generic_fallbacks() {
        let options = IconOptions {
            pixels: Some(std::sync::Arc::new(pixmap(24, |_, _| [10, 20, 30, 255]))),
            ..IconOptions::default()
        };

        let built = build(&options, FALLBACKS[0], 24, &test_theme([0; 3]));

        assert!(built.source.starts_with("pixmap "), "{}", built.source);
        assert!(!built.fallback);
    }

    #[test]
    fn an_invalid_published_file_gives_way_to_the_pixmap() {
        let root = test_root("invalid-primary");
        let broken = svg_at(&root, "broken", "not an svg");
        let options = IconOptions {
            path: Some(broken.to_string_lossy().into_owned()),
            pixels: Some(std::sync::Arc::new(pixmap(24, |_, _| [10, 20, 30, 255]))),
            ..IconOptions::default()
        };

        let built = build(&options, FALLBACKS[0], 24, &test_theme([0; 3]));

        assert!(built.source.starts_with("pixmap "), "{}", built.source);
        assert!(!built.fallback);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_item_id_is_tried_before_the_generic_fallback() {
        let Some(themed) = FALLBACKS
            .iter()
            .copied()
            .find(|name| lookup(name, 24).is_some())
        else {
            return;
        };

        let built = build(&IconOptions::default(), themed, 24, &test_theme([0; 3]));

        assert!(built.source.starts_with(&format!("id {themed} ->")));
        assert!(
            built.fallback,
            "an inferred icon must keep retrying published artwork"
        );
    }

    #[test]
    fn an_unknown_or_unsafe_item_id_uses_the_generic_fallback() {
        for item_id in [
            format!("status-hub-unknown-id-{}", std::process::id()),
            "../application-default".to_owned(),
        ] {
            let built = build(&IconOptions::default(), &item_id, 24, &test_theme([0; 3]));

            assert!(built.source.starts_with("GENERIC "), "{}", built.source);
            assert!(built.fallback);
        }
    }

    #[test]
    fn a_published_icon_keeps_priority_over_the_item_id() {
        let Some(themed) = FALLBACKS
            .iter()
            .copied()
            .find(|name| lookup(name, 24).is_some())
        else {
            return;
        };
        let options = IconOptions {
            pixels: Some(std::sync::Arc::new(pixmap(24, |_, _| [10, 20, 30, 255]))),
            ..IconOptions::default()
        };

        let built = build(&options, themed, 24, &test_theme([0; 3]));

        assert!(built.source.starts_with("pixmap "), "{}", built.source);
        assert!(!built.fallback);
    }

    #[test]
    fn an_explicit_symbolic_name_is_trusted_even_with_multiple_colours() {
        let root = test_root("explicit-symbolic");
        let path = svg_at(
            &root,
            "explicit-symbolic",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\"><rect width=\"8\" height=\"16\" fill=\"red\"/><rect x=\"8\" width=\"8\" height=\"16\" fill=\"blue\"/></svg>",
        );

        let (handle, policy) = test_file(path, "explicit-symbolic", 16, &light_panel());

        assert!(!handle.symbolic);
        assert_eq!(&raster_pixels(&handle).2[..3], &light_panel().ink);
        assert_eq!(policy, "symbolic-explicit");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_multicolour_svg_is_rasterized_without_recolouring() {
        let root = test_root("painted-vector");
        let path = svg_at(
            &root,
            "painted-vector",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" width=\"16\" height=\"16\">\
             <rect width=\"8\" height=\"16\" fill=\"red\"/>\
             <rect x=\"8\" width=\"8\" height=\"16\" fill=\"blue\"/></svg>",
        );

        let (handle, policy) = test_file(path, "painted-vector", 16, &light_panel());

        assert!(!handle.symbolic);
        assert_eq!(policy, "original-coloured");
        assert_eq!(&raster_pixels(&handle).2[..4], &[255, 0, 0, 255]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_multicolour_svg_too_large_to_tint_keeps_its_original_artwork() {
        let root = test_root("oversized-vector");
        let filler = "<rect width=\"1\" height=\"1\" fill=\"red\"/>".repeat(9000);
        let path = svg_at(
            &root,
            "oversized",
            &format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" width=\"16\" height=\"16\">\
                 <rect width=\"8\" height=\"16\" fill=\"red\"/>\
                 <rect x=\"8\" width=\"8\" height=\"16\" fill=\"blue\"/>{filler}</svg>"
            ),
        );
        assert!(std::fs::metadata(&path).unwrap().len() > 256 * 1024);

        let (handle, policy) = test_file(path, "oversized", 16, &light_panel());

        assert!(!handle.symbolic);
        assert_eq!(policy, "original-unprocessed");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_regular_svg_keeps_its_published_colours_in_original_mode() {
        let root = test_root("original-vector");
        let path = svg_at(
            &root,
            "regular",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"16\" height=\"16\" fill=\"red\"/></svg>",
        );

        let (handle, policy) = test_file(path, "regular", 16, &original_icons());

        assert!(!handle.symbolic);
        assert_eq!(policy, "original-disabled");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_explicit_symbolic_svg_still_follows_the_panel_in_original_mode() {
        let root = test_root("original-symbolic");
        let path = svg_at(
            &root,
            "regular-symbolic",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"16\" height=\"16\"/></svg>",
        );

        let (handle, policy) = test_file(path, "regular-symbolic", 16, &original_icons());

        assert!(!handle.symbolic);
        assert_eq!(&raster_pixels(&handle).2[..3], &original_icons().ink);
        assert_eq!(policy, "symbolic-explicit");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_pixmap_keeps_its_published_colours_in_original_mode() {
        let options = IconOptions {
            pixels: Some(std::sync::Arc::new(pixmap(10, |_, _| [220, 30, 40, 255]))),
            ..IconOptions::default()
        };

        let built = build(&options, "", 10, &original_icons());

        assert_eq!(built.paint, "original-disabled");
        assert!(!built.handle.symbolic);
    }

    #[test]
    fn a_monochrome_raster_is_adapted_regardless_of_where_it_was_found() {
        let root = test_root("payload-raster");
        let glyph = pixmap(10, |_, y| {
            if (2..8).contains(&y) {
                [245, 245, 245, 180]
            } else {
                [0, 0, 0, 0]
            }
        });
        let path = root.join("tray.png");
        png_at(&path, &glyph);

        for source in ["payload", "published"] {
            let built = from_file(
                path.clone(),
                "",
                source.to_owned(),
                10,
                &light_panel(),
                IconKind::Primary,
            )
            .unwrap();
            assert_eq!(built.paint, "monotone");
            assert_eq!(built.source, source);
            assert_eq!(
                &raster_pixels(&built.handle).2[3 * 10 * 4..3 * 10 * 4 + 3],
                &light_panel().ink
            );
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vector_artwork_from_a_payload_still_goes_through_symbolic_detection() {
        let root = test_root("payload-vector");
        let path = svg_at(
            &root,
            "glyph-symbolic",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\"><rect width=\"16\" height=\"16\"/></svg>",
        );

        let (handle, paint) = test_file(path, "glyph-symbolic", 16, &light_panel());

        assert!(!handle.symbolic);
        assert_eq!(&raster_pixels(&handle).2[..3], &light_panel().ink);
        assert_eq!(paint, "symbolic-explicit");

        std::fs::remove_dir_all(root).unwrap();
    }
}
