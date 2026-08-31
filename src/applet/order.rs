use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet, CosmicConfigEntry, Error};

use crate::applet::pins::CONFIG_VERSION;
use crate::core::OrderStore;
use crate::core::model::{ItemKey, TrayItem};
use crate::core::ordering::MAX_REMEMBERED;

pub const ORDER_KEY: &str = "order";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Order {
    keys: Vec<ItemKey>,
}

impl Order {
    pub fn from_keys(keys: impl IntoIterator<Item = ItemKey>) -> Self {
        let mut ordered: Vec<ItemKey> = Vec::new();
        for key in keys {
            if ordered.len() >= MAX_REMEMBERED {
                break;
            }
            if !ordered.contains(&key) {
                ordered.push(key);
            }
        }
        Self { keys: ordered }
    }

    pub fn keys(&self) -> &[ItemKey] {
        &self.keys
    }

    pub fn into_keys(self) -> Vec<ItemKey> {
        self.keys
    }
}

pub fn normalise(draft: &[ItemKey], items: &[TrayItem]) -> Vec<ItemKey> {
    let mut ordered: Vec<ItemKey> = draft
        .iter()
        .filter(|key| items.iter().any(|item| &&item.key == key))
        .cloned()
        .collect();

    for item in items {
        if !ordered.contains(&item.key) {
            ordered.push(item.key.clone());
        }
    }

    ordered
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn row_at(offset: f32, row: u16, spacing: u16, rows: usize) -> Option<usize> {
    if rows == 0 || !offset.is_finite() {
        return None;
    }
    let pitch = f32::from(row.saturating_add(spacing)).max(1.0);
    let index = (offset.max(0.0) / pitch).floor() as usize;
    Some(index.min(rows - 1))
}

pub fn move_to(order: &mut Vec<ItemKey>, key: &ItemKey, index: usize) -> bool {
    let Some(from) = order.iter().position(|entry| entry == key) else {
        return false;
    };
    let to = index.min(order.len().saturating_sub(1));
    if from == to {
        return false;
    }

    let key = order.remove(from);
    order.insert(to, key);
    true
}

impl CosmicConfigEntry for Order {
    const VERSION: u64 = CONFIG_VERSION;

    fn write_entry(&self, config: &Config) -> Result<(), Error> {
        config.set(ORDER_KEY, self.keys())
    }

    fn get_entry(config: &Config) -> Result<Self, (Vec<Error>, Self)> {
        match config.get::<Vec<ItemKey>>(ORDER_KEY) {
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
            if key.as_ref() != ORDER_KEY {
                continue;
            }
            match config.get::<Vec<ItemKey>>(ORDER_KEY) {
                Ok(keys) => {
                    let next = Self::from_keys(keys);
                    if *self != next {
                        *self = next;
                        updated.push(ORDER_KEY);
                    }
                }
                Err(error) => errors.push(error),
            }
        }

        (errors, updated)
    }
}

pub struct ConfigOrderStore {
    config: Option<Config>,
    written: Vec<ItemKey>,
}

impl ConfigOrderStore {
    pub fn open(app_id: &str) -> Self {
        match Config::new(app_id, CONFIG_VERSION) {
            Ok(config) => Self {
                config: Some(config),
                written: Vec::new(),
            },
            Err(error) => {
                tracing::warn!(%error, "the order will not persist, the config is unavailable");
                Self {
                    config: None,
                    written: Vec::new(),
                }
            }
        }
    }
}

impl OrderStore for ConfigOrderStore {
    fn load(&self) -> Vec<ItemKey> {
        let Some(config) = self.config.as_ref() else {
            return Vec::new();
        };

        match config.get::<Vec<ItemKey>>(ORDER_KEY) {
            Ok(keys) => Order::from_keys(keys).into_keys(),
            Err(error) => {
                tracing::debug!(%error, "no order stored yet");
                Vec::new()
            }
        }
    }

    fn store(&mut self, order: &[ItemKey]) {
        let Some(config) = self.config.as_ref() else {
            return;
        };
        if self.written == order {
            return;
        }

        match config.set(ORDER_KEY, order) {
            Ok(()) => self.written = order.to_vec(),
            Err(error) => tracing::warn!(%error, "could not store the item order"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::item;

    fn key(id: &str) -> ItemKey {
        ItemKey::new(id.to_owned(), 0)
    }

    fn keys(order: &[ItemKey]) -> Vec<String> {
        order.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_draft_keeps_the_order_the_user_gave_it() {
        let items = [item("chat", 0), item("music", 1), item("vpn", 2)];
        let draft = [key("vpn"), key("chat"), key("music")];

        assert_eq!(keys(&normalise(&draft, &items)), ["vpn", "chat", "music"]);
    }

    #[test]
    fn an_item_that_appears_while_the_settings_are_open_joins_the_end() {
        let items = [item("chat", 0), item("music", 1), item("vpn", 2)];
        let draft = [key("vpn"), key("chat")];

        assert_eq!(keys(&normalise(&draft, &items)), ["vpn", "chat", "music"]);
    }

    #[test]
    fn an_item_that_quit_leaves_the_draft() {
        let items = [item("chat", 0), item("vpn", 1)];
        let dropped = [key("vpn"), key("music"), key("chat")];

        assert_eq!(keys(&normalise(&dropped, &items)), ["vpn", "chat"]);
    }

    #[test]
    fn a_pointer_picks_the_row_it_is_over() {
        assert_eq!(row_at(0.0, 36, 8, 3), Some(0));
        assert_eq!(row_at(35.0, 36, 8, 3), Some(0));
        assert_eq!(row_at(44.0, 36, 8, 3), Some(1));
        assert_eq!(row_at(88.0, 36, 8, 3), Some(2));
    }

    #[test]
    fn a_pointer_past_either_end_sticks_to_the_row_there() {
        assert_eq!(row_at(-40.0, 36, 8, 3), Some(0));
        assert_eq!(row_at(4000.0, 36, 8, 3), Some(2));
        assert_eq!(row_at(10.0, 36, 8, 0), None);
        assert_eq!(row_at(f32::NAN, 36, 8, 3), None);
    }

    #[test]
    fn dropping_a_row_shifts_the_rest_along() {
        let mut order = vec![key("chat"), key("music"), key("vpn")];

        assert!(move_to(&mut order, &key("vpn"), 0));
        assert_eq!(keys(&order), ["vpn", "chat", "music"]);

        assert!(move_to(&mut order, &key("vpn"), 2));
        assert_eq!(
            keys(&order),
            ["chat", "music", "vpn"],
            "the rest closes the gap rather than swapping with the far end"
        );
    }

    #[test]
    fn a_drag_that_has_not_left_its_row_changes_nothing() {
        let mut order = vec![key("chat"), key("music")];

        assert!(!move_to(&mut order, &key("chat"), 0));
        assert!(!move_to(&mut order, &key("absent"), 1));
        assert_eq!(keys(&order), ["chat", "music"]);
    }

    #[test]
    fn moving_the_last_item_up_and_the_second_item_to_first_keeps_the_expected_order() {
        let mut order = vec![key("chat"), key("music"), key("vpn")];
        assert!(move_to(&mut order, &key("vpn"), 1));
        assert_eq!(keys(&order), ["chat", "vpn", "music"]);

        assert!(move_to(&mut order, &key("vpn"), 0));
        assert_eq!(keys(&order), ["vpn", "chat", "music"]);
    }

    #[test]
    fn a_stored_order_drops_duplicates_and_stops_at_the_remembered_limit() {
        let repeated = Order::from_keys([key("chat"), key("music"), key("chat")]);
        assert_eq!(keys(repeated.keys()), ["chat", "music"]);

        let many: Vec<ItemKey> = (0..MAX_REMEMBERED + 8)
            .map(|index| key(&format!("app{index}")))
            .collect();
        assert_eq!(Order::from_keys(many).keys().len(), MAX_REMEMBERED);
    }
}
