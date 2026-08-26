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
                let handle = self
                    .entries
                    .remove(&key)
                    .unwrap_or_else(|| build(&options, size));
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

fn build(options: &IconOptions, size: u16) -> icon::Handle {
    if let Some(path) = &options.path {
        let path = PathBuf::from(path);
        if path.exists() {
            return icon::from_path(path);
        }
    }

    if let Some(name) = &options.name {
        let extra: Vec<PathBuf> = options
            .theme_path
            .iter()
            .map(|path| PathBuf::from(path.as_str()))
            .collect();

        let published = if extra.is_empty() {
            None
        } else {
            lookup(name, &extra, size)
        };
        let themed = lookup(name, &[], size);

        if let Some(path) = themed.or(published) {
            return handle_for(path, name);
        }
    }

    if let Some(image) = &options.pixels {
        return icon::from_raster_pixels(image.width, image.height, image.bytes.clone());
    }

    for fallback in FALLBACKS {
        if let Some(path) = lookup(fallback, &[], size) {
            return handle_for(path, fallback);
        }
    }

    icon::from_name(FALLBACKS[0]).size(size).handle()
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
