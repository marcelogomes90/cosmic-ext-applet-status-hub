use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use cosmic::widget::icon::{self, Named};

use crate::core::icons::{IconKind, IconOptions, RgbaImage, resolve};
use crate::core::model::{Generation, ItemAddress, TraySnapshot};

mod paint;
mod svg;
#[cfg(test)]
mod testing;

use self::paint::prepare_raster;
use self::svg::{render_svg, single_ink_svg};

const FALLBACKS: [&str; 2] = ["application-default", "application-x-executable"];

const MAX_RASTER_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    address: ItemAddress,
    generation: Generation,
    kind: IconKind,
    size: u16,
}

#[derive(Debug)]
struct Entry {
    handle: icon::Handle,
    fallback: bool,
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

        let mut next = HashMap::with_capacity(snapshot.items.len() * 2);
        let mut unresolved_primary = false;

        for item in &snapshot.items {
            for kind in [IconKind::Primary, IconKind::Overlay] {
                let options = resolve(&item.icon, item.status, kind, u32::from(size));

                if kind == IconKind::Overlay && options.is_empty() {
                    continue;
                }

                let key = Key {
                    address: item.address.clone(),
                    generation: item.generation,
                    kind,
                    size,
                };
                let entry = match self.entries.remove(&key) {
                    Some(entry) if !entry.fallback || !retry_fallbacks => entry,
                    _ => {
                        let built = build(&options, size, theme);
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
                            "icon resolved"
                        );
                        Entry {
                            handle: built.handle,
                            fallback: built.fallback,
                        }
                    }
                };
                if kind == IconKind::Primary && entry.fallback {
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
            .map(|entry| &entry.handle)
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

fn build(options: &IconOptions, size: u16, theme: &ThemeContext) -> Built {
    if let Some(name) = &options.name {
        if let Some(path) = lookup(name, size) {
            let source = format!("name {name} -> {}", path.display());
            return from_file(path, name, Origin::Published, source, size, theme);
        }

        if let Some((path, origin)) = options
            .theme_path
            .as_deref()
            .and_then(|root| lookup_published(name, root, size))
        {
            let source = format!("{} name {name} -> {}", origin.label(), path.display());
            return from_file(path, name, origin, source, size, theme);
        }
    }

    if let Some(published) = &options.path
        && let Some((path, origin)) = resolve_path(published)
    {
        let source = format!("{} path {}", origin.label(), path.display());
        return from_file(path, "", origin, source, size, theme);
    }

    if let Some(published) = &options.pixels {
        let recoloured = theme
            .colour_icons
            .then(|| prepare_raster(published, size, theme))
            .flatten();
        let note = if recoloured.is_some() {
            " recoloured"
        } else {
            ""
        };
        let image = recoloured.as_ref().unwrap_or(published.as_ref());
        let source = format!("pixmap {}x{}{note}", image.width, image.height);
        return Built {
            handle: icon::from_raster_pixels(image.width, image.height, image.bytes.clone()),
            source,
            fallback: false,
            paint: if recoloured.is_some() {
                "pixmap-recoloured"
            } else {
                "original"
            },
        };
    }

    for fallback in FALLBACKS {
        if let Some(path) = lookup(fallback, size) {
            let source = format!("GENERIC {fallback} -> {}", path.display());
            let (mut handle, mut paint) = handle_for(path, fallback, size, theme);
            if !theme.colour_icons {
                handle.symbolic = true;
                paint = "symbolic-fallback";
            }
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
        paint: "original",
    }
}

fn from_file(
    path: PathBuf,
    name: &str,
    origin: Origin,
    source: String,
    size: u16,
    theme: &ThemeContext,
) -> Built {
    let (handle, paint) = handle_from(path, name, origin, size, theme);
    Built {
        handle,
        source,
        fallback: false,
        paint,
    }
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
    icon_theme: String,
    colour_icons: bool,
}

fn theme_context(colour_icons: bool) -> ThemeContext {
    let theme = cosmic::theme::active();
    let container = theme.cosmic().background(theme.transparent);
    let ink = container.on.into_format::<u8, u8>();
    ThemeContext {
        ink: [ink.red, ink.green, ink.blue],
        icon_theme: cosmic::icon_theme::default(),
        colour_icons,
    }
}

fn handle_from(
    path: PathBuf,
    name: &str,
    origin: Origin,
    size: u16,
    theme: &ThemeContext,
) -> (icon::Handle, &'static str) {
    if theme.colour_icons
        && let Some(image) = raster(&path)
        && let Some(recoloured) = prepare_raster(&image, size, theme)
    {
        return (
            icon::from_raster_pixels(recoloured.width, recoloured.height, recoloured.bytes),
            if origin == Origin::Payload {
                "payload-recoloured"
            } else {
                "published-recoloured"
            },
        );
    }
    handle_for(path, name, size, theme)
}

fn raster(path: &Path) -> Option<RgbaImage> {
    if path.extension() == Some(OsStr::new("svg"))
        || !std::fs::metadata(path).is_ok_and(|meta| meta.len() <= MAX_RASTER_BYTES)
    {
        return None;
    }

    let decoded = image::open(path).ok()?.into_rgba8();
    Some(RgbaImage {
        width: decoded.width(),
        height: decoded.height(),
        bytes: decoded.into_raw(),
    })
}

fn handle_for(
    path: PathBuf,
    name: &str,
    size: u16,
    theme: &ThemeContext,
) -> (icon::Handle, &'static str) {
    let explicit = name.ends_with("-symbolic")
        || path
            .file_stem()
            .and_then(OsStr::to_str)
            .is_some_and(|stem| stem.ends_with("-symbolic"));
    if !theme.colour_icons {
        let mut handle = icon::from_path(path);
        handle.symbolic = explicit;
        return (
            handle,
            if explicit {
                "symbolic-explicit"
            } else {
                "original"
            },
        );
    }

    let vector = path.extension() == Some(OsStr::new("svg"));
    let inferred = !explicit && single_ink_svg(&path);
    if vector
        && !explicit
        && !inferred
        && let Some(image) = render_svg(&path, size)
        && let Some(recoloured) = prepare_raster(&image, size, theme)
    {
        return (
            icon::from_raster_pixels(recoloured.width, recoloured.height, recoloured.bytes),
            "tinted-detailed",
        );
    }
    let mut handle = icon::from_path(path);
    handle.symbolic |= explicit || inferred || vector;
    let paint = if explicit {
        "symbolic-explicit"
    } else if inferred {
        "symbolic-inferred"
    } else if vector {
        "symbolic-painted"
    } else {
        "original"
    };
    (handle, paint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::applet::icons::testing::*;
    use crate::core::model::Pixmap;
    use crate::core::testing::item;

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

        let built = build(&options, 24, &test_theme([0; 3]));

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

        let built = build(&options, 24, &test_theme([0; 3]));

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

        let built = build(&options, 24, &test_theme([0; 3]));

        assert_eq!(built.source, format!("published path {}", path.display()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_pixmap_wins_over_the_generic_fallbacks() {
        let options = IconOptions {
            pixels: Some(std::sync::Arc::new(pixmap(24, |_, _| [10, 20, 30, 255]))),
            ..IconOptions::default()
        };

        let built = build(&options, 24, &test_theme([0; 3]));

        assert!(built.source.starts_with("pixmap "), "{}", built.source);
        assert!(!built.fallback);
    }

    #[test]
    fn an_explicit_symbolic_name_is_trusted_even_with_multiple_colours() {
        let root = test_root("explicit-symbolic");
        let path = svg_at(
            &root,
            "explicit-symbolic",
            "<svg><path fill=\"red\"/><path fill=\"blue\"/></svg>",
        );

        let (handle, policy) = handle_for(path, "explicit-symbolic", 16, &light_panel());

        assert!(handle.symbolic);
        assert_eq!(policy, "symbolic-explicit");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_multicolour_svg_is_tinted_without_becoming_symbolic() {
        let root = test_root("painted-vector");
        let path = svg_at(
            &root,
            "painted-vector",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" width=\"16\" height=\"16\">\
             <rect width=\"8\" height=\"16\" fill=\"red\"/>\
             <rect x=\"8\" width=\"8\" height=\"16\" fill=\"blue\"/></svg>",
        );

        let (handle, policy) = handle_for(path, "painted-vector", 16, &light_panel());

        assert!(!handle.symbolic);
        assert_eq!(policy, "tinted-detailed");
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

        let (handle, policy) = handle_for(path, "regular", 16, &original_icons());

        assert!(!handle.symbolic);
        assert_eq!(policy, "original");
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

        let (handle, policy) = handle_for(path, "regular-symbolic", 16, &original_icons());

        assert!(handle.symbolic);
        assert_eq!(policy, "symbolic-explicit");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_pixmap_keeps_its_published_colours_in_original_mode() {
        let options = IconOptions {
            pixels: Some(std::sync::Arc::new(pixmap(10, |_, _| [220, 30, 40, 255]))),
            ..IconOptions::default()
        };

        let built = build(&options, 10, &original_icons());

        assert_eq!(built.paint, "original");
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

        let (_, payload) = handle_from(path.clone(), "", Origin::Payload, 10, &light_panel());
        let (_, published) = handle_from(path, "", Origin::Published, 10, &light_panel());

        assert_eq!(
            payload, "payload-recoloured",
            "a payload raster follows the panel theme"
        );
        assert_eq!(
            published, "published-recoloured",
            "a published raster follows the same content-based rule"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vector_artwork_from_a_payload_still_goes_through_symbolic_detection() {
        let root = test_root("payload-vector");
        let path = svg_at(&root, "glyph-symbolic", "<svg/>");

        let (handle, paint) =
            handle_from(path, "glyph-symbolic", Origin::Payload, 16, &light_panel());

        assert!(handle.symbolic);
        assert_eq!(paint, "symbolic-explicit");

        std::fs::remove_dir_all(root).unwrap();
    }
}
