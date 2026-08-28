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

#[derive(Debug)]
struct Entry {
    handle: icon::Handle,
    fallback: bool,
}

#[derive(Debug, Default)]
pub struct IconCache {
    entries: HashMap<Key, Entry>,
}

impl IconCache {
    pub fn refresh(&mut self, snapshot: &TraySnapshot, size: u16, retry_fallbacks: bool) -> bool {
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
                        let built = build(&options, size);
                        tracing::info!(
                            item = %item.id,
                            ?kind,
                            size,
                            name = options.name.as_deref().unwrap_or("-"),
                            theme_path = options.theme_path.as_deref().unwrap_or("-"),
                            pixmap = options.pixels.is_some(),
                            source = %built.source,
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
}

fn build(options: &IconOptions, size: u16) -> Built {
    if let Some(path) = &options.path {
        let path = PathBuf::from(path);
        if path.exists() {
            let source = format!("published path {}", path.display());
            return Built {
                handle: icon::from_path(path),
                source,
                fallback: false,
            };
        }
    }

    if let Some(name) = &options.name {
        if let Some(path) = options
            .theme_path
            .as_deref()
            .and_then(|root| lookup_published(name, root, size))
        {
            let source = format!("published name {name} -> {}", path.display());
            return Built {
                handle: handle_for(path, name),
                source,
                fallback: false,
            };
        }

        if let Some(path) = lookup(name, size) {
            let source = format!("name {name} -> {}", path.display());
            return Built {
                handle: handle_for(path, name),
                source,
                fallback: false,
            };
        }
    }

    if let Some(image) = &options.pixels {
        let source = format!("pixmap {}x{}", image.width, image.height);
        return Built {
            handle: icon::from_raster_pixels(image.width, image.height, image.bytes.clone()),
            source,
            fallback: false,
        };
    }

    for fallback in FALLBACKS {
        if let Some(path) = lookup(fallback, size) {
            let source = format!("GENERIC {fallback} -> {}", path.display());
            return Built {
                handle: handle_for(path, fallback),
                source,
                fallback: true,
            };
        }
    }

    Built {
        handle: icon::from_name(FALLBACKS[0]).size(size).handle(),
        source: "GENERIC unresolved".to_owned(),
        fallback: true,
    }
}

fn lookup_published(name: &str, root: &str, size: u16) -> Option<PathBuf> {
    if root.is_empty() {
        return None;
    }
    let root = PathBuf::from(root);
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

fn handle_for(path: PathBuf, name: &str) -> icon::Handle {
    let mut handle = icon::from_path(path);
    handle.symbolic |= name.ends_with("-symbolic");
    handle
}

#[cfg(test)]
mod tests {
    use super::*;
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
            Some(icon_path)
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
}
