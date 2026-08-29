use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet, CosmicConfigEntry, Error};

use crate::core::model::{ItemKey, TrayItem};

pub const MAX_PINNED: usize = 16;

pub const CONFIG_VERSION: u64 = 1;
pub const PINNED_KEY: &str = "pinned";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pins {
    pinned: Vec<ItemKey>,
}

impl Pins {
    pub fn from_keys(keys: impl IntoIterator<Item = ItemKey>) -> Self {
        let mut pinned: Vec<ItemKey> = Vec::new();
        for key in keys {
            if pinned.len() >= MAX_PINNED {
                break;
            }
            if !pinned.contains(&key) {
                pinned.push(key);
            }
        }
        Self { pinned }
    }

    pub fn contains(&self, key: &ItemKey) -> bool {
        self.pinned.contains(key)
    }

    pub fn toggle(&mut self, key: &ItemKey) -> bool {
        if let Some(position) = self.pinned.iter().position(|pinned| pinned == key) {
            self.pinned.remove(position);
            return true;
        }
        if self.pinned.len() >= MAX_PINNED {
            tracing::info!(item = %key, "not pinning, the panel is full");
            return false;
        }
        self.pinned.push(key.clone());
        true
    }

    pub fn keys(&self) -> &[ItemKey] {
        &self.pinned
    }
}

pub fn partition_for_panel<'a>(
    items: &'a [TrayItem],
    pins: &Pins,
    panel_capacity: usize,
) -> (Vec<&'a TrayItem>, Vec<&'a TrayItem>) {
    let mut panel = Vec::with_capacity(panel_capacity.min(items.len()));
    let mut popup = Vec::with_capacity(items.len());

    for item in items {
        if pins.contains(&item.key) && panel.len() < panel_capacity {
            panel.push(item);
        } else {
            popup.push(item);
        }
    }

    (panel, popup)
}

impl CosmicConfigEntry for Pins {
    const VERSION: u64 = CONFIG_VERSION;

    fn write_entry(&self, config: &Config) -> Result<(), Error> {
        config.set(PINNED_KEY, self.keys())
    }

    fn get_entry(config: &Config) -> Result<Self, (Vec<Error>, Self)> {
        match config.get::<Vec<ItemKey>>(PINNED_KEY) {
            Ok(keys) => Ok(Self::from_keys(keys)),
            Err(error) => Err((vec![error], Self::default())),
        }
    }

    fn update_keys<T: AsRef<str>>(
        &mut self,
        config: &Config,
        changed_keys: &[T],
    ) -> (Vec<Error>, Vec<&'static str>) {
        let mut errors = Vec::new();
        let mut updated = Vec::new();

        for key in changed_keys {
            if key.as_ref() != PINNED_KEY {
                continue;
            }
            match config.get::<Vec<ItemKey>>(PINNED_KEY) {
                Ok(keys) => {
                    let next = Self::from_keys(keys);
                    if *self != next {
                        *self = next;
                        updated.push(PINNED_KEY);
                    }
                }
                Err(error) => errors.push(error),
            }
        }

        (errors, updated)
    }
}

pub struct PinStore {
    config: Option<Config>,
}

impl PinStore {
    pub fn open(app_id: &str) -> Self {
        match Config::new(app_id, CONFIG_VERSION) {
            Ok(config) => Self {
                config: Some(config),
            },
            Err(error) => {
                tracing::warn!(%error, "pins will not persist, the config is unavailable");
                Self { config: None }
            }
        }
    }

    pub fn load(&self) -> Pins {
        let Some(config) = self.config.as_ref() else {
            return Pins::default();
        };

        match config.get::<Vec<ItemKey>>(PINNED_KEY) {
            Ok(keys) => Pins::from_keys(keys),
            Err(error) => {
                tracing::debug!(%error, "no pins stored yet");
                Pins::default()
            }
        }
    }

    pub fn save(&self, pins: &Pins) {
        let Some(config) = self.config.as_ref() else {
            return;
        };

        if let Err(error) = config.set(PINNED_KEY, pins.keys()) {
            tracing::warn!(%error, "could not store the pinned items");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::item;

    fn key(id: &str) -> ItemKey {
        ItemKey::new(id, 0)
    }

    fn ids(items: &[&TrayItem]) -> Vec<String> {
        items.iter().map(|item| item.key.id.clone()).collect()
    }

    #[test]
    fn pinning_the_same_item_twice_unpins_it() {
        let mut pins = Pins::default();

        assert!(pins.toggle(&key("steam")));
        assert!(pins.contains(&key("steam")));

        assert!(pins.toggle(&key("steam")));
        assert!(!pins.contains(&key("steam")));
    }

    #[test]
    fn the_panel_never_takes_more_than_its_share() {
        let mut pins = Pins::from_keys((0..MAX_PINNED).map(|n| key(&format!("item{n}"))));

        assert!(!pins.toggle(&key("one-too-many")));
        assert_eq!(pins.keys().len(), MAX_PINNED);

        assert!(pins.toggle(&key("item0")));
        assert!(pins.toggle(&key("one-too-many")));
    }

    #[test]
    fn a_repeated_key_is_only_remembered_once() {
        let pins = Pins::from_keys([key("steam"), key("steam"), key("discord")]);

        assert_eq!(pins.keys(), [key("steam"), key("discord")]);
    }

    #[test]
    fn partitioning_keeps_both_halves_in_snapshot_order() {
        let items = vec![item("steam", 1), item("discord", 2), item("telegram", 3)];
        let pins = Pins::from_keys([key("telegram"), key("steam")]);

        let (panel, popup) = partition_for_panel(&items, &pins, usize::MAX);

        assert_eq!(ids(&panel), ["steam", "telegram"]);
        assert_eq!(ids(&popup), ["discord"]);
    }

    #[test]
    fn panel_capacity_keeps_excess_pins_available_in_the_popup() {
        let items = vec![item("steam", 1), item("discord", 2), item("telegram", 3)];
        let pins = Pins::from_keys([key("steam"), key("discord"), key("telegram")]);

        let (panel, popup) = partition_for_panel(&items, &pins, 1);

        assert_eq!(ids(&panel), ["steam"]);
        assert_eq!(ids(&popup), ["discord", "telegram"]);
    }

    #[test]
    fn zero_panel_capacity_reserves_every_item_for_the_popup() {
        let items = vec![item("steam", 1), item("discord", 2)];
        let pins = Pins::from_keys([key("steam"), key("discord")]);

        let (panel, popup) = partition_for_panel(&items, &pins, 0);

        assert!(panel.is_empty());
        assert_eq!(ids(&popup), ["steam", "discord"]);
    }

    #[test]
    fn pins_survive_a_round_trip_through_the_config() {
        let path = std::env::temp_dir().join(format!(
            "cosmic-status-hub-pins-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);

        let config = Config::with_custom_path("test-pins", CONFIG_VERSION, path.clone())
            .expect("a config under a temporary path");
        let store = PinStore {
            config: Some(config),
        };

        let pins = Pins::from_keys([key("steam"), ItemKey::new("chat", 1)]);
        store.save(&pins);

        assert_eq!(store.load(), pins);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn a_changed_key_is_reported_only_when_the_value_really_moved() {
        let path = std::env::temp_dir().join(format!(
            "cosmic-status-hub-entry-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);

        let config = Config::with_custom_path("test-entry", CONFIG_VERSION, path.clone())
            .expect("a config under a temporary path");

        let stored = Pins::from_keys([key("steam")]);
        stored.write_entry(&config).expect("the entry writes");

        let mut live = Pins::default();
        let (errors, changed) = live.update_keys(&config, &[PINNED_KEY]);
        assert!(errors.is_empty());
        assert_eq!(changed, [PINNED_KEY]);
        assert_eq!(live, stored);

        let (_, unchanged) = live.update_keys(&config, &[PINNED_KEY]);
        assert!(unchanged.is_empty(), "an identical reload is not a change");

        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn an_unavailable_config_simply_has_no_pins() {
        let store = PinStore { config: None };

        assert!(store.load().keys().is_empty());
        store.save(&Pins::from_keys([key("steam")]));
    }

    #[test]
    fn a_pin_for_an_item_that_is_gone_is_kept_but_not_shown() {
        let items = vec![item("steam", 1)];
        let pins = Pins::from_keys([key("steam"), key("an-app-that-is-not-running")]);

        let (panel, popup) = partition_for_panel(&items, &pins, usize::MAX);

        assert_eq!(ids(&panel), ["steam"]);
        assert!(popup.is_empty());
        assert_eq!(pins.keys().len(), 2, "the absent pin is remembered");
    }
}
