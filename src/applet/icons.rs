use std::collections::HashMap;
use std::path::PathBuf;

use cosmic::widget::icon::{self, Named};

use crate::core::icons::{IconKind, IconOptions, resolve};
use crate::core::model::{Generation, ItemAddress, TraySnapshot};

const FALLBACKS: [&str; 2] = ["application-default", "application-x-executable"];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    address: ItemAddress,
    generation: Generation,
    kind: IconKind,
    size: u16,
}

#[derive(Debug, Default)]
pub struct IconCache {
    entries: HashMap<Key, icon::Handle>,
}

impl IconCache {
    pub fn refresh(&mut self, snapshot: &TraySnapshot, size: u16) {
        let mut next = HashMap::with_capacity(snapshot.items.len() * 2);

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
                let handle = self.entries.remove(&key).unwrap_or_else(|| {
                    let (handle, source) = build(&options, size);
                    tracing::info!(
                        item = %item.id,
                        ?kind,
                        size,
                        name = options.name.as_deref().unwrap_or("-"),
                        theme_path = options.theme_path.as_deref().unwrap_or("-"),
                        pixmap = options.pixels.is_some(),
                        %source,
                        "icon resolved"
                    );
                    handle
                });
                next.insert(key, handle);
            }
        }

        self.entries = next;
    }

    pub fn get(
        &self,
        address: &ItemAddress,
        generation: Generation,
        kind: IconKind,
        size: u16,
    ) -> Option<&icon::Handle> {
        self.entries.get(&Key {
            address: address.clone(),
            generation,
            kind,
            size,
        })
    }
}

fn build(options: &IconOptions, size: u16) -> (icon::Handle, String) {
    if let Some(path) = &options.path {
        let path = PathBuf::from(path);
        if path.exists() {
            let source = format!("published path {}", path.display());
            return (icon::from_path(path), source);
        }
    }

    if let Some(name) = &options.name {
        let extra: Vec<PathBuf> = options
            .theme_path
            .iter()
            .filter(|path| !path.is_empty())
            .map(|path| PathBuf::from(path.as_str()))
            .collect();

        if let Some(path) = lookup(name, &extra, size) {
            let source = format!("name {name} -> {}", path.display());
            return (handle_for(path, name), source);
        }
    }

    if let Some(image) = &options.pixels {
        let source = format!("pixmap {}x{}", image.width, image.height);
        return (
            icon::from_raster_pixels(image.width, image.height, image.bytes.clone()),
            source,
        );
    }

    for fallback in FALLBACKS {
        if let Some(path) = lookup(fallback, &[], size) {
            let source = format!("GENERIC {fallback} -> {}", path.display());
            return (handle_for(path, fallback), source);
        }
    }

    (
        icon::from_name(FALLBACKS[0]).size(size).handle(),
        "GENERIC unresolved".to_owned(),
    )
}

fn lookup(name: &str, extra: &[PathBuf], size: u16) -> Option<PathBuf> {
    named(name, extra)
        .size(size)
        .prefer_svg(true)
        .path()
        .or_else(|| named(name, extra).prefer_svg(false).path())
}

fn named(name: &str, extra: &[PathBuf]) -> Named {
    icon::from_name(name.to_owned()).with_extra_paths(extra.to_vec())
}

fn handle_for(path: PathBuf, name: &str) -> icon::Handle {
    let mut handle = icon::from_path(path);
    handle.symbolic |= name.ends_with("-symbolic");
    handle
}
