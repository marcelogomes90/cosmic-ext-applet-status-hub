use crate::core::model::{ItemKey, TrayItem};

pub const MAX_REMEMBERED: usize = 64;

pub fn sort_items(items: &mut [TrayItem], remembered: &[ItemKey]) {
    items.sort_by_cached_key(|item| {
        let position = remembered
            .iter()
            .position(|key| key == &item.key)
            .unwrap_or(usize::MAX);
        (position, item.discovery_seq, item.key.clone())
    });
}

pub fn remembered_after(baseline: &[ItemKey], sorted_items: &[TrayItem]) -> Vec<ItemKey> {
    let mut merged: Vec<ItemKey> = sorted_items.iter().map(|item| item.key.clone()).collect();
    let absent: Vec<ItemKey> = baseline
        .iter()
        .filter(|key| !merged.contains(key))
        .cloned()
        .collect();
    merged.extend(absent);
    merged.truncate(MAX_REMEMBERED);
    merged
}

pub fn assign_dup_indices(ids_in_discovery_order: &[&str]) -> Vec<u16> {
    let mut seen: Vec<(&str, u16)> = Vec::new();
    ids_in_discovery_order
        .iter()
        .map(|id| {
            if let Some((_, count)) = seen.iter_mut().find(|(seen_id, _)| seen_id == id) {
                *count = count.saturating_add(1);
                *count
            } else {
                seen.push((id, 0));
                0
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::item;

    fn keys(items: &[TrayItem]) -> Vec<String> {
        items.iter().map(|i| i.key.id.clone()).collect()
    }

    #[test]
    fn without_memory_order_follows_discovery() {
        let mut items = vec![item("telegram", 3), item("steam", 1), item("discord", 2)];
        sort_items(&mut items, &[]);
        assert_eq!(keys(&items), ["steam", "discord", "telegram"]);
    }

    #[test]
    fn discovery_order_beats_insertion_order() {
        let mut a = vec![item("steam", 1), item("discord", 2), item("nextcloud", 3)];
        let mut b = vec![item("nextcloud", 3), item("steam", 1), item("discord", 2)];
        sort_items(&mut a, &[]);
        sort_items(&mut b, &[]);
        assert_eq!(keys(&a), keys(&b));
    }

    #[test]
    fn remembered_order_wins_over_discovery() {
        let remembered = vec![
            ItemKey::new("steam", 0),
            ItemKey::new("discord", 0),
            ItemKey::new("telegram", 0),
        ];
        let mut items = vec![item("telegram", 1), item("discord", 2), item("steam", 3)];
        sort_items(&mut items, &remembered);
        assert_eq!(keys(&items), ["steam", "discord", "telegram"]);
    }

    #[test]
    fn unknown_items_go_after_remembered_ones_in_discovery_order() {
        let remembered = vec![ItemKey::new("steam", 0)];
        let mut items = vec![item("nextcloud", 9), item("steam", 5), item("vpn", 7)];
        sort_items(&mut items, &remembered);
        assert_eq!(keys(&items), ["steam", "vpn", "nextcloud"]);
    }

    #[test]
    fn removing_an_item_preserves_the_rest() {
        let remembered = vec![
            ItemKey::new("a", 0),
            ItemKey::new("b", 0),
            ItemKey::new("c", 0),
            ItemKey::new("d", 0),
        ];
        let mut items = vec![item("d", 4), item("c", 3), item("a", 1)];
        sort_items(&mut items, &remembered);
        assert_eq!(keys(&items), ["a", "c", "d"]);
    }

    #[test]
    fn remembering_puts_live_items_first_and_keeps_absent_ones() {
        let baseline = vec![ItemKey::new("steam", 0), ItemKey::new("nextcloud", 0)];
        let mut items = vec![item("discord", 2), item("steam", 1)];
        sort_items(&mut items, &baseline);

        assert_eq!(
            remembered_after(&baseline, &items),
            vec![
                ItemKey::new("steam", 0),
                ItemKey::new("discord", 0),
                ItemKey::new("nextcloud", 0),
            ]
        );
    }

    #[test]
    fn remembering_is_unaffected_by_which_item_resolved_first() {
        let both = {
            let mut items = vec![item("a", 0), item("b", 1)];
            sort_items(&mut items, &[]);
            remembered_after(&[], &items)
        };

        let partial = remembered_after(&[], &[item("b", 1)]);
        assert_eq!(partial, vec![ItemKey::new("b", 0)]);

        let recomputed = {
            let mut items = vec![item("b", 1), item("a", 0)];
            sort_items(&mut items, &[]);
            remembered_after(&partial, &items)
        };
        assert_eq!(recomputed, both);
        assert_eq!(recomputed, vec![ItemKey::new("a", 0), ItemKey::new("b", 0)]);
    }

    #[test]
    fn remembering_is_bounded() {
        let baseline: Vec<ItemKey> = (0..MAX_REMEMBERED + 10)
            .map(|i| ItemKey::new(format!("old-{i}"), 0))
            .collect();
        let items = vec![item("live", 1)];
        let merged = remembered_after(&baseline, &items);
        assert_eq!(merged.len(), MAX_REMEMBERED);
        assert_eq!(
            merged[0],
            ItemKey::new("live", 0),
            "live items are never evicted"
        );
    }

    #[test]
    fn duplicate_ids_get_successive_indices() {
        assert_eq!(
            assign_dup_indices(&["steam", "discord", "steam", "steam", "vpn"]),
            [0, 0, 1, 2, 0]
        );
    }

    #[test]
    fn dup_indices_depend_only_on_discovery_order() {
        assert_eq!(assign_dup_indices(&["a", "a", "b"]), [0, 1, 0]);
        assert_eq!(assign_dup_indices(&["a", "a", "b"]), [0, 1, 0]);
    }
}
