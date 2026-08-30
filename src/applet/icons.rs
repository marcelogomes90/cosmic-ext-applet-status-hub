use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use cosmic::widget::icon::{self, Named};

use crate::core::icons::{IconKind, IconOptions, RgbaImage, resolve};
use crate::core::model::{Generation, ItemAddress, TraySnapshot};

const FALLBACKS: [&str; 2] = ["application-default", "application-x-executable"];

const MAX_SVG_BYTES: u64 = 256 * 1024;

const MAX_RASTER_BYTES: u64 = 1024 * 1024;

const MAX_CHROMA: f32 = 8.0;

const MIN_ALPHA: u8 = 16;

const MAX_PIXEL_CHROMA: u8 = 8;

const MAX_TONE_SPAN: u16 = 16;

const MAX_TONAL_OFFSET: i16 = 80;

const MIN_CLEAR_PERCENT: usize = 10;

const MIN_CONTRAST: f32 = 3.0;

const MIN_LEGIBLE_PERCENT: usize = 15;

const LEGIBLE_MAJORITY_PERCENT: usize = 50;

const MIN_LEGIBLE_EXTENT_PERCENT: usize = 60;

const MAX_BADGE_BOX_PERCENT: usize = 60;

const MIN_BADGE_FILL_PERCENT: usize = 15;

const COLOUR_KEYS: [&str; 6] = [
    "fill",
    "stroke",
    "color",
    "stop-color",
    "flood-color",
    "lighting-color",
];

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
    pub fn refresh(&mut self, snapshot: &TraySnapshot, size: u16, retry_fallbacks: bool) -> bool {
        self.refresh_with_theme(snapshot, size, retry_fallbacks, theme_context())
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
            let (handle, paint) = handle_for(path, name);
            return Built {
                handle,
                source,
                fallback: false,
                paint,
            };
        }

        if let Some((path, origin)) = options
            .theme_path
            .as_deref()
            .and_then(|root| lookup_published(name, root, size))
        {
            let source = format!("{} name {name} -> {}", origin.label(), path.display());
            let (handle, paint) = handle_from(path, name, origin, theme);
            return Built {
                handle,
                source,
                fallback: false,
                paint,
            };
        }
    }

    if let Some(published) = &options.path
        && let Some((path, origin)) = resolve_path(published)
    {
        let source = format!("{} path {}", origin.label(), path.display());
        let (handle, paint) = handle_from(path, "", origin, theme);
        return Built {
            handle,
            source,
            fallback: false,
            paint,
        };
    }

    if let Some(published) = &options.pixels {
        let recoloured = recolour(published, theme);
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
            let (handle, paint) = handle_for(path, fallback);
            return Built {
                handle,
                source,
                fallback: true,
                paint,
            };
        }
    }

    Built {
        handle: icon::from_name(FALLBACKS[0]).size(size).handle(),
        source: "GENERIC unresolved".to_owned(),
        fallback: true,
        paint: "original",
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
    background: [u8; 3],
    icon_theme: String,
}

fn theme_context() -> ThemeContext {
    let theme = cosmic::theme::active();
    let container = theme.cosmic().background(theme.transparent);
    let ink = container.on.into_format::<u8, u8>();
    let background = container.base.into_format::<u8, u8>();
    ThemeContext {
        ink: [ink.red, ink.green, ink.blue],
        background: [background.red, background.green, background.blue],
        icon_theme: cosmic::icon_theme::default(),
    }
}

fn recolour(image: &RgbaImage, theme: &ThemeContext) -> Option<RgbaImage> {
    let width = usize::try_from(image.width).ok()?;
    let height = usize::try_from(image.height).ok()?;
    let pixels = image.bytes.as_chunks::<4>().0;
    if width == 0 || height == 0 || pixels.len() != width * height {
        return None;
    }

    let badge = match classify_badge(pixels, width, height)? {
        Badge::Absent => None,
        Badge::Protected(bounds) => Some(bounds),
    };
    let stats = tone_stats(pixels, width, badge, theme.background)?;
    if stats.reads_on_its_own() {
        return None;
    }
    let representative = stats.representative;

    let tonal = stats.span.1 - stats.span.0 > MAX_TONE_SPAN;
    if badge.is_some() && tonal {
        return None;
    }
    let representative_level = i16::try_from(level([
        representative[0],
        representative[1],
        representative[2],
        u8::MAX,
    ]))
    .unwrap_or_default();
    let mut bytes = image.bytes.clone();
    for (index, pixel) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = index % width;
        let y = index / width;
        if pixel[3] >= MIN_ALPHA && badge.is_none_or(|bounds| !bounds.contains(x, y)) {
            let alpha = pixel[3];
            if tonal {
                let offset = tonal_offset(level(*pixel), representative_level);
                for (channel, ink) in pixel[..3].iter_mut().zip(theme.ink) {
                    *channel =
                        u8::try_from((i16::from(ink) + offset).clamp(0, 255)).unwrap_or_default();
                }
            } else {
                *pixel = [theme.ink[0], theme.ink[1], theme.ink[2], alpha];
            }
        }
    }

    Some(RgbaImage {
        width: image.width,
        height: image.height,
        bytes,
    })
}

fn tonal_offset(source: u16, representative: i16) -> i16 {
    i16::try_from(source)
        .unwrap_or(i16::MAX)
        .saturating_sub(representative)
        .clamp(-MAX_TONAL_OFFSET, MAX_TONAL_OFFSET)
}

enum Badge {
    Absent,
    Protected(Bounds),
}

fn classify_badge(pixels: &[[u8; 4]], width: usize, height: usize) -> Option<Badge> {
    let mut clear = 0usize;
    let mut visible = 0usize;
    let mut badge: Option<Bounds> = None;
    let mut coloured = 0usize;

    for (index, &pixel) in pixels.iter().enumerate() {
        if pixel[3] < MIN_ALPHA {
            clear += 1;
            continue;
        }

        let low = pixel[0].min(pixel[1]).min(pixel[2]);
        let high = pixel[0].max(pixel[1]).max(pixel[2]);
        if high - low > MAX_PIXEL_CHROMA {
            coloured += 1;
            let x = index % width;
            let y = index / width;
            badge = Some(match badge {
                Some(bounds) => bounds.include(x, y),
                None => Bounds::new(x, y),
            });
        }
        visible += 1;
    }

    if visible == 0 || clear * 100 < pixels.len() * MIN_CLEAR_PERCENT {
        return None;
    }

    match badge {
        Some(bounds) if plausible_badge(bounds, coloured, width, height) => {
            Some(Badge::Protected(bounds.expand(width, height)))
        }
        Some(_) => None,
        None => Some(Badge::Absent),
    }
}

struct ToneStats {
    span: (u16, u16),
    representative: [u8; 3],
    legible: usize,
    extent: usize,
}

impl ToneStats {
    fn reads_on_its_own(&self) -> bool {
        self.legible >= LEGIBLE_MAJORITY_PERCENT
            || (self.legible >= MIN_LEGIBLE_PERCENT && self.extent >= MIN_LEGIBLE_EXTENT_PERCENT)
    }
}

#[derive(Default)]
struct Extent(Option<Bounds>);

impl Extent {
    fn include(&mut self, x: usize, y: usize) {
        self.0 = Some(match self.0 {
            Some(bounds) => bounds.include(x, y),
            None => Bounds::new(x, y),
        });
    }

    fn area(&self) -> usize {
        self.0.map_or(0, |bounds| bounds.width() * bounds.height())
    }
}

fn tone_stats(
    pixels: &[[u8; 4]],
    width: usize,
    badge: Option<Bounds>,
    background: [u8; 3],
) -> Option<ToneStats> {
    let eligible = |index: usize, pixel: [u8; 4]| {
        pixel[3] >= MIN_ALPHA
            && badge.is_none_or(|bounds| !bounds.contains(index % width, index / width))
    };
    let peak = pixels
        .iter()
        .enumerate()
        .filter(|&(index, &pixel)| eligible(index, pixel))
        .map(|(_, pixel)| pixel[3])
        .max()?;

    let background = cosmic::iced::Color::from_rgb8(background[0], background[1], background[2]);
    let mut span: Option<(u16, u16)> = None;
    let mut alpha = 0u64;
    let mut legible = 0u64;
    let mut channels = [0u64; 3];
    let mut visible = Extent::default();
    let mut readable = Extent::default();
    for (index, &pixel) in pixels.iter().enumerate() {
        if !eligible(index, pixel) {
            continue;
        }
        let (x, y) = (index % width, index / width);
        visible.include(x, y);

        let pixel_level = level(pixel);
        span = Some(match span {
            Some((low, high)) => (low.min(pixel_level), high.max(pixel_level)),
            None => (pixel_level, pixel_level),
        });

        if u16::from(pixel[3]) * 10 < u16::from(peak) * 9 {
            continue;
        }

        let weight = u64::from(pixel[3]);
        alpha += weight;
        if cosmic::iced::Color::from_rgb8(pixel[0], pixel[1], pixel[2])
            .relative_contrast(background)
            >= MIN_CONTRAST
        {
            legible += weight;
            readable.include(x, y);
        }
        for channel in 0..3 {
            channels[channel] += u64::from(pixel[channel]) * weight;
        }
    }

    if alpha == 0 {
        return None;
    }
    Some(ToneStats {
        span: span?,
        representative: channels.map(|channel| u8::try_from(channel / alpha).unwrap_or(u8::MAX)),
        legible: usize::try_from(legible * 100 / alpha).unwrap_or(100),
        extent: readable.area() * 100 / visible.area().max(1),
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

    fn contains(self, x: usize, y: usize) -> bool {
        (self.min_x..=self.max_x).contains(&x) && (self.min_y..=self.max_y).contains(&y)
    }

    fn expand(self, width: usize, height: usize) -> Self {
        let margin = width.max(height).div_ceil(32).max(1);
        Self {
            min_x: self.min_x.saturating_sub(margin),
            min_y: self.min_y.saturating_sub(margin),
            max_x: self.max_x.saturating_add(margin).min(width - 1),
            max_y: self.max_y.saturating_add(margin).min(height - 1),
        }
    }
}

fn plausible_badge(bounds: Bounds, coloured: usize, width: usize, height: usize) -> bool {
    let area = bounds.width() * bounds.height();
    let edge_margin = width.max(height).div_ceil(16).max(1);
    let at_edge = bounds.min_x <= edge_margin
        || bounds.min_y <= edge_margin
        || bounds.max_x.saturating_add(edge_margin) >= width - 1
        || bounds.max_y.saturating_add(edge_margin) >= height - 1;

    at_edge
        && area * 100 <= width * height * MAX_BADGE_BOX_PERCENT
        && coloured * 100 >= area * MIN_BADGE_FILL_PERCENT
}

fn level(pixel: [u8; 4]) -> u16 {
    (u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2])) / 3
}

fn handle_from(
    path: PathBuf,
    name: &str,
    origin: Origin,
    theme: &ThemeContext,
) -> (icon::Handle, &'static str) {
    if origin == Origin::Payload
        && let Some(image) = raster(&path)
        && let Some(recoloured) = recolour(&image, theme)
    {
        return (
            icon::from_raster_pixels(recoloured.width, recoloured.height, recoloured.bytes),
            "payload-recoloured",
        );
    }
    handle_for(path, name)
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

fn handle_for(path: PathBuf, name: &str) -> (icon::Handle, &'static str) {
    let explicit = name.ends_with("-symbolic")
        || path
            .file_stem()
            .and_then(OsStr::to_str)
            .is_some_and(|stem| stem.ends_with("-symbolic"));
    let inferred = !explicit && single_ink_svg(&path);
    let mut handle = icon::from_path(path);
    handle.symbolic |= explicit || inferred;
    let paint = if explicit {
        "symbolic-explicit"
    } else if inferred {
        "symbolic-inferred"
    } else {
        "original"
    };
    (handle, paint)
}

fn single_ink_svg(path: &Path) -> bool {
    if path.extension() != Some(OsStr::new("svg"))
        || std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_SVG_BYTES)
    {
        return false;
    }

    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };

    if text.contains("<image") || text.contains("data:image/") {
        return false;
    }

    let mut ink = None;
    for value in colours(&live_rules(&text)) {
        match paint(value) {
            Paint::Ignore => {}
            Paint::Ink(next) if ink.is_none_or(|current| current == next) => ink = Some(next),
            Paint::Ink(_) | Paint::Unsupported => return false,
        }
    }

    true
}

fn colours(text: &str) -> impl Iterator<Item = &str> {
    COLOUR_KEYS.iter().flat_map(move |key| values_of(text, key))
}

fn values_of<'a>(text: &'a str, key: &'a str) -> impl Iterator<Item = &'a str> {
    text.match_indices(key).filter_map(move |(at, _)| {
        let leading = text[..at].chars().next_back();
        if leading.is_some_and(|ch| ch.is_alphanumeric() || ch == '-' || ch == '_') {
            return None;
        }

        let rest = text[at + key.len()..].trim_start();
        let rest = rest
            .strip_prefix('=')
            .or_else(|| rest.strip_prefix(':'))?
            .trim_start();
        Some(value_of(rest))
    })
}

fn live_rules(text: &str) -> String {
    let used: Vec<&str> = values_of(text, "class")
        .flat_map(str::split_whitespace)
        .collect();

    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let (head, tail) = rest.split_at(open);
        let Some(close) = tail.find('}') else {
            break;
        };

        out.push_str(head);
        let selector = head.rsplit(['}', '>', ';']).next().unwrap_or("").trim();
        if selects_something(selector, &used) {
            out.push_str(&tail[..=close]);
        }
        rest = &tail[close + 1..];
    }

    out.push_str(rest);
    out
}

fn selects_something(selector: &str, used: &[&str]) -> bool {
    selector.split(',').any(|part| {
        let part = part.trim();
        match part.strip_prefix('.') {
            Some(class) if !class.contains([' ', '.', '#', '[', ':', '>']) => used.contains(&class),
            _ => true,
        }
    })
}

fn value_of(rest: &str) -> &str {
    let quoted = rest.starts_with(['"', '\'']);
    let rest = if quoted { &rest[1..] } else { rest };

    let mut depth = 0u32;
    for (at, ch) in rest.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            '"' | '\'' | ';' | '}' | '>' => return &rest[..at],
            _ if !quoted && ch.is_whitespace() => return &rest[..at],
            _ => {}
        }
    }

    rest
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Paint {
    Ignore,
    Ink([u32; 3]),
    Unsupported,
}

fn grey(level: u8) -> Paint {
    Paint::Ink([f32::from(level).to_bits(); 3])
}

fn paint(value: &str) -> Paint {
    let value = value.trim().to_ascii_lowercase();

    match value.as_str() {
        "none" | "transparent" | "inherit" | "currentcolor" => return Paint::Ignore,
        "black" => return grey(0),
        "white" => return grey(255),
        "silver" => return grey(192),
        "gray" | "grey" => return grey(128),
        "gainsboro" => return grey(220),
        "whitesmoke" => return grey(245),
        "lightgray" | "lightgrey" => return grey(211),
        "darkgray" | "darkgrey" => return grey(169),
        "dimgray" | "dimgrey" => return grey(105),
        _ => {}
    }

    let channels = if let Some(hex) = value.strip_prefix('#') {
        hex_channels(hex)
    } else if let Some(args) = value
        .strip_prefix("rgb(")
        .or_else(|| value.strip_prefix("rgba("))
    {
        rgb_channels(args)
    } else {
        None
    };

    let Some([red, green, blue]) = channels else {
        return Paint::Unsupported;
    };
    if ![red, green, blue]
        .into_iter()
        .all(|channel| channel.is_finite() && (0.0..=255.0).contains(&channel))
    {
        return Paint::Unsupported;
    }
    let low = red.min(green).min(blue);
    let high = red.max(green).max(blue);
    if high - low <= MAX_CHROMA {
        Paint::Ink([
            red.round().to_bits(),
            green.round().to_bits(),
            blue.round().to_bits(),
        ])
    } else {
        Paint::Unsupported
    }
}

fn hex_channels(hex: &str) -> Option<[f32; 3]> {
    if !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }

    let channel = |text: &str| u8::from_str_radix(text, 16).ok().map(f32::from);
    match hex.len() {
        3 | 4 => {
            let doubled: String = hex.chars().take(3).flat_map(|ch| [ch, ch]).collect();
            rgb_from(&doubled, channel)
        }
        6 | 8 => rgb_from(hex, channel),
        _ => None,
    }
}

fn rgb_from(hex: &str, channel: impl Fn(&str) -> Option<f32>) -> Option<[f32; 3]> {
    Some([
        channel(hex.get(0..2)?)?,
        channel(hex.get(2..4)?)?,
        channel(hex.get(4..6)?)?,
    ])
}

fn rgb_channels(args: &str) -> Option<[f32; 3]> {
    let mut channels = args
        .trim_end_matches(')')
        .split([',', ' ', '/'])
        .filter(|channel| !channel.is_empty())
        .map(|channel| match channel.strip_suffix('%') {
            Some(percent) => percent.parse::<f32>().ok().map(|value| value * 2.55),
            None => channel.parse::<f32>().ok(),
        });

    Some([channels.next()??, channels.next()??, channels.next()??])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Pixmap;
    use crate::core::testing::item;

    fn test_root(suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("status-hub-icon-{}-{suffix}", std::process::id()))
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

        assert!(cache.refresh(&snapshot, 24, false));
        let icon_path = icon_dir.join(format!("{name}.svg"));
        std::fs::write(icon_path, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
        assert!(!cache.refresh(&snapshot, 24, true));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn colour_or_icon_theme_changes_rebuild_cached_pixmaps() {
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

        let first_theme = test_theme([0; 3], [255; 3]);
        cache.refresh_with_theme(&snapshot, 24, false, first_theme.clone());
        let first = hash(&cache);
        cache.refresh_with_theme(&snapshot, 24, false, first_theme.clone());
        assert_eq!(hash(&cache), first, "an unchanged theme reuses the handle");

        let mut other_icons = first_theme.clone();
        other_icons.icon_theme = "other".to_owned();
        cache.refresh_with_theme(&snapshot, 24, false, other_icons);
        let second = hash(&cache);
        assert_ne!(second, first, "an icon theme change rebuilds the handle");

        cache.refresh_with_theme(&snapshot, 24, false, test_theme([255; 3], [0; 3]));
        assert_ne!(
            hash(&cache),
            second,
            "a colour theme change rebuilds the handle"
        );
    }

    fn svg_at(root: &PathBuf, name: &str, body: &str) -> PathBuf {
        std::fs::create_dir_all(root).unwrap();
        let path = root.join(format!("{name}.svg"));
        std::fs::write(&path, body).unwrap();
        path
    }

    fn published_svg(root: &Path, name: &str, body: &str) -> PathBuf {
        let icon_dir = root.join("hicolor/scalable/apps");
        std::fs::create_dir_all(&icon_dir).unwrap();
        let path = icon_dir.join(format!("{name}.svg"));
        std::fs::write(&path, body).unwrap();
        path
    }

    fn test_theme(ink: [u8; 3], background: [u8; 3]) -> ThemeContext {
        ThemeContext {
            ink,
            background,
            icon_theme: "test".to_owned(),
        }
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

        let built = build(&options, 24, &test_theme([0; 3], [255; 3]));

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

        let built = build(&options, 24, &test_theme([0; 3], [255; 3]));

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

        let built = build(&options, 24, &test_theme([0; 3], [255; 3]));

        assert_eq!(built.source, format!("published path {}", path.display()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_pixmap_wins_over_the_generic_fallbacks() {
        let options = IconOptions {
            pixels: Some(std::sync::Arc::new(pixmap(24, |_, _| [10, 20, 30, 255]))),
            ..IconOptions::default()
        };

        let built = build(&options, 24, &test_theme([0; 3], [255; 3]));

        assert!(built.source.starts_with("pixmap "), "{}", built.source);
        assert!(!built.fallback);
    }

    #[test]
    fn a_single_ink_svg_is_symbolic_even_without_the_suffix() {
        let root = test_root("single-ink");
        let cases = [
            ("bare", "<svg><path d=\"M0 0\"/></svg>"),
            ("black", "<svg><path fill=\"#000000\" stroke='none'/></svg>"),
            ("current", "<svg><path fill=\"currentColor\"/></svg>"),
            (
                "css",
                "<svg><style>.a{fill:#222;stroke:none}</style><path class=\"a\"/></svg>",
            ),
            ("functional", "<svg><path fill=\"rgb(40, 40, 40)\"/></svg>"),
            ("tinted-grey", "<svg><path fill=\"#232629\"/></svg>"),
            (
                "percentages",
                "<svg><path fill=\"rgb(20%,20%,20%)\"/></svg>",
            ),
            (
                "equivalent",
                "<svg><path fill=\"#222\"/><path stroke=\"rgb(34,34,34)\"/></svg>",
            ),
        ];

        for (name, body) in cases {
            assert!(
                single_ink_svg(&svg_at(&root, name, body)),
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
            "<svg><path fill=\"#111\"/><path stroke=\"#eee\"/></svg>",
        );

        assert!(!single_ink_svg(&path));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_explicit_symbolic_name_is_trusted_even_with_multiple_colours() {
        let root = test_root("explicit-symbolic");
        let path = svg_at(
            &root,
            "explicit-symbolic",
            "<svg><path fill=\"red\"/><path fill=\"blue\"/></svg>",
        );

        let (handle, policy) = handle_for(path, "explicit-symbolic");

        assert!(handle.symbolic);
        assert_eq!(policy, "symbolic-explicit");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unused_coloured_style_rule_does_not_change_the_live_single_ink() {
        let root = test_root("dead-rules");
        let stylesheet = "<style>.ColorScheme-Text { color:#dfdfdf; } \
                          .ColorScheme-Highlight { color:#4285f4; }</style>";
        let body = format!(
            "<svg>{stylesheet}<path style=\"fill:currentColor\" class=\"ColorScheme-Text\"/></svg>"
        );

        assert!(single_ink_svg(&svg_at(&root, "unused-highlight", &body)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_live_coloured_style_rule_keeps_the_svg_original() {
        let root = test_root("live-rules");
        let stylesheet = "<style>.ColorScheme-Text { color:#dfdfdf; } \
                          .ColorScheme-Highlight { color:#4285f4; }</style>";
        let body = format!(
            "<svg>{stylesheet}<path style=\"fill:currentColor\" class=\"ColorScheme-Highlight\"/></svg>"
        );

        assert!(!single_ink_svg(&svg_at(&root, "used-highlight", &body)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn coloured_or_embedded_content_is_preserved() {
        let root = test_root("coloured");
        let cases = [
            ("hex", "<svg><path fill=\"#4caf50\"/></svg>"),
            ("named", "<svg><path fill=\"red\"/></svg>"),
            (
                "gradient",
                "<svg><stop stop-color=\"#f00\"/><path fill=\"url(#g)\"/></svg>",
            ),
            (
                "mixed",
                "<svg><path fill=\"#000\"/><path stroke=\"#08f\"/></svg>",
            ),
            (
                "raster",
                "<svg><image href=\"data:image/png;base64,AAA\"/></svg>",
            ),
        ];

        for (name, body) in cases {
            assert!(
                !single_ink_svg(&svg_at(&root, name, body)),
                "expected {name} to keep its original appearance"
            );
        }

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

    fn dark_panel() -> ThemeContext {
        test_theme([231, 231, 231], [27, 27, 27])
    }

    fn light_panel() -> ThemeContext {
        test_theme([46, 52, 54], [251, 251, 251])
    }

    fn pixmap(side: u32, fill: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
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

    fn alphas(image: &RgbaImage) -> Vec<u8> {
        image
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3])
            .collect()
    }

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
    fn a_pixmap_that_already_contrasts_is_left_alone() {
        let light = pixmap(10, |_, y| {
            if (2..8).contains(&y) {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let dark = pixmap(10, |_, y| {
            if (2..8).contains(&y) {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });

        assert!(recolour(&light, &dark_panel()).is_none());
        assert!(recolour(&dark, &light_panel()).is_none());
        assert!(recolour(&light, &light_panel()).is_some());
        assert!(recolour(&dark, &dark_panel()).is_some());
    }

    #[test]
    fn a_coloured_badge_is_preserved_while_the_single_ink_base_is_recoloured() {
        let badge = [0, 180, 255, 255];
        let image = pixmap(10, |x, y| match (x, y) {
            (8, 4) => [255, 255, 255, 255],
            (7.., 2..8) => badge,
            (_, 2..8) => [255, 255, 255, 255],
            _ => [0, 0, 0, 0],
        });
        let before = alphas(&image);

        let out = recolour(&image, &light_panel()).expect("the badge is a protected region");
        let pixels = out.bytes.as_chunks::<4>().0;

        assert_eq!(alphas(&out), before);
        assert_eq!(&pixels[3 * 10 + 2][..3], &light_panel().ink);
        assert_eq!(pixels[3 * 10 + 8], badge);
        assert_eq!(pixels[4 * 10 + 8], [255, 255, 255, 255]);
    }

    #[test]
    fn a_badge_on_multi_tone_artwork_keeps_the_whole_pixmap_original() {
        let image = pixmap(10, |x, y| match (x, y) {
            (7.., 2..8) => [220, 20, 40, 255],
            (_, 2..5) => [250, 250, 250, 255],
            (_, 5..8) => [20, 20, 20, 255],
            _ => [0, 0, 0, 0],
        });

        assert!(recolour(&image, &light_panel()).is_none());
    }

    #[test]
    fn an_achromatic_outline_keeps_its_tonal_structure_when_adapted() {
        let image = pixmap(12, |x, y| {
            if !(2..10).contains(&x) || !(2..10).contains(&y) {
                [0, 0, 0, 0]
            } else if (3..9).contains(&x) && (3..9).contains(&y) {
                [250, 250, 250, 255]
            } else {
                [230, 230, 230, 255]
            }
        });
        let before = alphas(&image);

        let out = recolour(&image, &light_panel()).expect("no tone in the artwork reads here");
        let pixels = out.bytes.as_chunks::<4>().0;
        let (outline, centre) = (pixels[2 * 12 + 2][0], pixels[4 * 12 + 4][0]);

        assert_eq!(alphas(&out), before);
        assert_eq!(
            i16::from(centre) - i16::from(outline),
            20,
            "the published tonal difference is carried over, not widened"
        );
        assert_eq!(centre, light_panel().ink[0] + 9);
        assert_eq!(outline, light_panel().ink[0] - 11);
    }

    #[test]
    fn legible_ink_has_to_trace_the_shape_not_just_sit_inside_it() {
        let outlined = pixmap(16, |x, y| {
            if !(2..14).contains(&x) || !(2..14).contains(&y) {
                [0, 0, 0, 0]
            } else if (3..13).contains(&x) && (3..13).contains(&y) {
                [250, 250, 250, 255]
            } else {
                [10, 10, 10, 255]
            }
        });
        let detailed = pixmap(16, |x, y| {
            if !(2..14).contains(&x) || !(2..14).contains(&y) {
                [0, 0, 0, 0]
            } else if (7..9).contains(&x) {
                [10, 10, 10, 255]
            } else {
                [250, 250, 250, 255]
            }
        });

        assert!(
            recolour(&outlined, &light_panel()).is_none(),
            "a dark outline carries the whole silhouette on a light panel"
        );
        assert!(
            recolour(&detailed, &light_panel()).is_some(),
            "a dark bar in the middle leaves the rest of the shape invisible"
        );
    }

    #[test]
    fn artwork_carrying_both_dark_and_light_ink_is_left_alone() {
        let image = pixmap(12, |x, y| {
            if !(2..10).contains(&x) || !(2..10).contains(&y) {
                [0, 0, 0, 0]
            } else if (3..9).contains(&x) && (3..9).contains(&y) {
                [10, 10, 10, 255]
            } else {
                [245, 245, 245, 255]
            }
        });

        assert!(recolour(&image, &light_panel()).is_none());
        assert!(recolour(&image, &dark_panel()).is_none());
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
    fn a_subtly_coloured_pixmap_is_preserved() {
        let image = pixmap(10, |_, y| {
            if (2..8).contains(&y) {
                [230, 240, 230, 255]
            } else {
                [0, 0, 0, 0]
            }
        });

        assert!(recolour(&image, &light_panel()).is_none());
    }

    #[test]
    fn a_multicolour_icon_is_not_mistaken_for_a_badge() {
        let image = pixmap(10, |x, y| {
            if !(2..8).contains(&y) {
                [0, 0, 0, 0]
            } else if x < 5 {
                [230, 20, 20, 255]
            } else {
                [20, 80, 230, 255]
            }
        });

        assert!(recolour(&image, &light_panel()).is_none());
    }

    #[test]
    fn a_pixmap_with_no_clear_margin_is_left_as_published() {
        let image = pixmap(10, |_, _| [128, 128, 128, 255]);

        assert!(recolour(&image, &light_panel()).is_none());
    }

    #[test]
    fn a_fully_transparent_pixmap_is_left_as_published() {
        let image = pixmap(10, |_, _| [0, 0, 0, 0]);

        assert!(recolour(&image, &light_panel()).is_none());
    }

    fn png_at(path: &Path, image: &RgbaImage) {
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

    #[test]
    fn a_raster_recovered_from_a_payload_is_adapted_to_the_panel() {
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

        let (_, payload) = handle_from(path.clone(), "", Origin::Payload, &light_panel());
        let (_, published) = handle_from(path, "", Origin::Published, &light_panel());

        assert_eq!(
            payload, "payload-recoloured",
            "the application drew this for its own panel, not for this theme"
        );
        assert_eq!(
            published, "original",
            "a file found where the application said it would be keeps its appearance"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn vector_artwork_from_a_payload_still_goes_through_symbolic_detection() {
        let root = test_root("payload-vector");
        let path = svg_at(&root, "glyph-symbolic", "<svg/>");

        let (handle, paint) = handle_from(path, "glyph-symbolic", Origin::Payload, &light_panel());

        assert!(handle.symbolic);
        assert_eq!(paint, "symbolic-explicit");

        std::fs::remove_dir_all(root).unwrap();
    }
}
