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
    let live: Vec<ItemKey> = sorted_items.iter().map(|item| item.key.clone()).collect();
    let mut merged: Vec<ItemKey> = Vec::with_capacity(baseline.len() + live.len());
    let mut showing = live.iter();

    for key in baseline {
        if live.contains(key) {
            for shown in showing.by_ref() {
                push_once(&mut merged, shown);
                if shown == key {
                    break;
                }
            }
        } else {
            push_once(&mut merged, key);
        }
    }
    for shown in showing {
        push_once(&mut merged, shown);
    }

    cap(merged, &live)
}

fn push_once(merged: &mut Vec<ItemKey>, key: &ItemKey) {
    if !merged.contains(key) {
        merged.push(key.clone());
    }
}

fn cap(mut merged: Vec<ItemKey>, live: &[ItemKey]) -> Vec<ItemKey> {
    if merged.len() <= MAX_REMEMBERED {
        return merged;
    }

    let mut absent_budget = MAX_REMEMBERED.saturating_sub(live.len());
    merged.retain(|key| {
        live.contains(key) || {
            let room = absent_budget > 0;
            absent_budget = absent_budget.saturating_sub(1);
            room
        }
    });
    merged.truncate(MAX_REMEMBERED);
    merged
}

pub fn merge_remembered(chosen: &[ItemKey], baseline: &[ItemKey]) -> Vec<ItemKey> {
    let mut merged: Vec<ItemKey> = Vec::with_capacity(chosen.len() + baseline.len());
    for key in chosen.iter().chain(baseline) {
        push_once(&mut merged, key);
    }
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
    fn an_absent_item_keeps_the_slot_it_had() {
        let baseline = vec![ItemKey::new("steam", 0), ItemKey::new("nextcloud", 0)];
        let mut items = vec![item("discord", 2), item("steam", 1)];
        sort_items(&mut items, &baseline);

        assert_eq!(
            remembered_after(&baseline, &items),
            vec![
                ItemKey::new("steam", 0),
                ItemKey::new("nextcloud", 0),
                ItemKey::new("discord", 0),
            ],
            "nextcloud is not running, but it was remembered before discord was ever seen"
        );
    }

    #[test]
    fn closing_an_application_does_not_move_it_to_the_back() {
        let chosen = vec![
            ItemKey::new("vpn", 0),
            ItemKey::new("chat", 0),
            ItemKey::new("music", 0),
        ];

        let mut still_running = vec![item("vpn", 1), item("music", 3)];
        sort_items(&mut still_running, &chosen);
        let while_closed = remembered_after(&chosen, &still_running);
        assert_eq!(while_closed, chosen, "the slot is held open");

        let mut back = vec![item("vpn", 1), item("music", 3), item("chat", 9)];
        sort_items(&mut back, &while_closed);
        assert_eq!(
            keys(&back),
            ["vpn", "chat", "music"],
            "it reopens where it was, not after everything discovered since"
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
    fn remembering_is_bounded_and_spends_the_room_on_what_is_running() {
        let baseline: Vec<ItemKey> = (0..MAX_REMEMBERED + 10)
            .map(|i| ItemKey::new(format!("old-{i}"), 0))
            .collect();
        let items = vec![item("live", 1)];
        let merged = remembered_after(&baseline, &items);

        assert_eq!(merged.len(), MAX_REMEMBERED);
        assert!(
            merged.contains(&ItemKey::new("live", 0)),
            "a running application never loses its slot to one that only left a slot behind"
        );
        assert_eq!(
            merged[0],
            ItemKey::new("old-0", 0),
            "the slots that survive are the earliest ones, in the order they were remembered"
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
