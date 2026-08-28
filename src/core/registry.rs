use std::collections::BTreeMap;
use std::sync::Arc;

use zbus::names::{BusName, UniqueName};
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use crate::core::lifecycle::{LifecycleState, Transition};
use crate::core::model::{
    Category, DiscoverySeq, Generation, IconSource, ItemAddress, ItemKey, ItemStatus, ToolTip,
    TrayItem, TraySnapshot, WatcherState,
};
use crate::core::ordering::{assign_dup_indices, sort_items};

#[derive(Clone, Debug, Default)]
pub struct ResolvedProps {
    pub id: String,
    pub title: String,
    pub category: Category,
    pub status: ItemStatus,
    pub menu_path: Option<OwnedObjectPath>,
    pub tooltip: Option<ToolTip>,
    pub icon: Arc<IconSource>,
}

#[derive(Clone, Debug)]
struct Entry {
    address: ItemAddress,
    generation: Generation,
    state: LifecycleState,
    props: ResolvedProps,
    resolved_once: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    Changed,
    Stale,
    Unknown,
}

#[derive(Debug, Default)]
pub struct Registry {
    entries: BTreeMap<DiscoverySeq, Entry>,
    next_seq: u64,
    next_generation: u64,
    remembered: Vec<ItemKey>,
    revision: u64,
}

impl Registry {
    pub fn new(remembered: Vec<ItemKey>) -> Self {
        Self {
            remembered,
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn remembered(&self) -> &[ItemKey] {
        &self.remembered
    }

    pub fn set_remembered(&mut self, remembered: Vec<ItemKey>) {
        if self.remembered != remembered {
            self.remembered = remembered;
            self.revision += 1;
        }
    }

    pub fn discover(&mut self, address: ItemAddress) -> Option<(DiscoverySeq, Generation)> {
        if self.entries.values().any(|entry| entry.address == address) {
            tracing::debug!(item = %address, "item already registered, ignoring re-announcement");
            return None;
        }

        let seq = DiscoverySeq(self.next_seq);
        self.next_seq += 1;
        let generation = self.next_generation();

        tracing::info!(
            item = %address,
            owner = %address.owner.as_str(),
            seq = seq.0,
            generation = %generation,
            "sni registered"
        );

        self.entries.insert(
            seq,
            Entry {
                address,
                generation,
                state: LifecycleState::Discovered.apply(&Transition::OwnerResolved),
                props: ResolvedProps::default(),
                resolved_once: false,
            },
        );
        self.revision += 1;

        Some((seq, generation))
    }

    pub fn begin_refresh(&mut self, seq: DiscoverySeq) -> Option<Generation> {
        let generation = self.next_generation();
        let entry = self.entries.get_mut(&seq)?;
        if matches!(entry.state, LifecycleState::Removing) {
            return None;
        }
        entry.generation = generation;
        entry.state = entry.state.apply(&Transition::Refresh);
        Some(generation)
    }

    pub fn apply_resolved(
        &mut self,
        seq: DiscoverySeq,
        generation: Generation,
        props: ResolvedProps,
    ) -> Applied {
        let Some(entry) = self.entries.get_mut(&seq) else {
            return Applied::Unknown;
        };
        if entry.generation != generation {
            tracing::debug!(
                item = %entry.address,
                arrived = %generation,
                current = %entry.generation,
                "stale generation ignored"
            );
            return Applied::Stale;
        }

        tracing::debug!(item = %entry.address, id = %props.id, "properties changed");
        entry.props = props;
        entry.state = entry.state.apply(&Transition::Resolved);
        entry.resolved_once = true;
        self.revision += 1;
        Applied::Changed
    }

    pub fn apply_failure(
        &mut self,
        seq: DiscoverySeq,
        generation: Generation,
        reason: String,
    ) -> Applied {
        let Some(entry) = self.entries.get_mut(&seq) else {
            return Applied::Unknown;
        };
        if entry.generation != generation {
            tracing::debug!(
                item = %entry.address,
                arrived = %generation,
                current = %entry.generation,
                "stale generation ignored"
            );
            return Applied::Stale;
        }

        tracing::warn!(item = %entry.address, reason = %reason, "item resolve failed");
        entry.state = entry.state.apply(&Transition::ResolveFailed(reason));
        entry.resolved_once = true;
        self.revision += 1;
        Applied::Changed
    }

    pub fn remove_for_lost_name(&mut self, name: &BusName<'_>) -> Vec<DiscoverySeq> {
        let doomed: Vec<DiscoverySeq> = self
            .entries
            .iter()
            .filter(|(_, entry)| match name {
                BusName::Unique(unique) => entry.address.owner.inner() == unique,
                BusName::WellKnown(well_known) => match entry.address.service.inner() {
                    BusName::WellKnown(service) => service == well_known,
                    BusName::Unique(_) => false,
                },
            })
            .map(|(seq, _)| *seq)
            .collect();

        for seq in &doomed {
            let entry = &self.entries[seq];
            tracing::info!(item = %entry.address, name = %name, "owner disappeared");
        }
        self.remove_slots(&doomed);
        doomed
    }

    pub fn remove_for_service(
        &mut self,
        service: &BusName<'_>,
        path: &ObjectPath<'_>,
    ) -> Vec<DiscoverySeq> {
        let doomed: Vec<DiscoverySeq> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.address.service.inner() == service && entry.address.path.as_ref() == *path
            })
            .map(|(seq, _)| *seq)
            .collect();

        for seq in &doomed {
            tracing::info!(item = %self.entries[seq].address, "sni removed");
        }
        self.remove_slots(&doomed);
        doomed
    }

    pub fn retain_addresses(&mut self, live: &[ItemAddress]) -> Vec<DiscoverySeq> {
        let doomed: Vec<DiscoverySeq> = self
            .entries
            .iter()
            .filter(|(_, entry)| !live.contains(&entry.address))
            .map(|(seq, _)| *seq)
            .collect();

        for seq in &doomed {
            tracing::info!(item = %self.entries[seq].address, "sni removed during reconcile");
        }
        self.remove_slots(&doomed);
        doomed
    }

    fn remove_slots(&mut self, slots: &[DiscoverySeq]) {
        for seq in slots {
            if let Some(mut entry) = self.entries.remove(seq) {
                entry.state = entry.state.apply(&Transition::OwnerLost);
            }
            self.revision += 1;
        }
    }

    pub fn props_of(&self, seq: DiscoverySeq) -> Option<&ResolvedProps> {
        self.entries.get(&seq).map(|entry| &entry.props)
    }

    pub fn generation_of(&self, seq: DiscoverySeq) -> Option<Generation> {
        self.entries.get(&seq).map(|entry| entry.generation)
    }

    pub fn address_of(&self, seq: DiscoverySeq) -> Option<&ItemAddress> {
        self.entries.get(&seq).map(|entry| &entry.address)
    }

    pub fn slot_for_address(&self, address: &ItemAddress) -> Option<DiscoverySeq> {
        self.entries
            .iter()
            .find(|(_, entry)| &entry.address == address)
            .map(|(seq, _)| *seq)
    }

    pub fn addresses(&self) -> Vec<ItemAddress> {
        self.entries
            .values()
            .map(|entry| entry.address.clone())
            .collect()
    }

    pub fn snapshot(&self, watcher: WatcherState) -> TraySnapshot {
        let visible: Vec<(&DiscoverySeq, &Entry)> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.state.is_visible() && entry.resolved_once)
            .collect();

        let stable_ids: Vec<String> = visible
            .iter()
            .map(|(_, entry)| {
                ItemKey::derive_id(
                    &entry.props.id,
                    &entry.props.title,
                    entry.address.service.inner(),
                )
            })
            .collect();
        let dups = assign_dup_indices(&stable_ids.iter().map(String::as_str).collect::<Vec<_>>());

        let mut items: Vec<TrayItem> = visible
            .iter()
            .zip(stable_ids.iter().zip(dups))
            .map(|((seq, entry), (id, dup))| TrayItem {
                address: entry.address.clone(),
                key: ItemKey::new(id.clone(), dup),
                generation: entry.generation,
                discovery_seq: **seq,
                state: entry.state.clone(),
                id: entry.props.id.clone(),
                title: entry.props.title.clone(),
                category: entry.props.category.clone(),
                status: entry.props.status,
                menu_path: entry.props.menu_path.clone(),
                tooltip: entry.props.tooltip.clone(),
                icon: entry.props.icon.clone(),
            })
            .collect();

        sort_items(&mut items, &self.remembered);

        TraySnapshot {
            items,
            watcher,
            revision: self.revision,
        }
    }

    fn next_generation(&mut self) -> Generation {
        self.next_generation += 1;
        Generation(self.next_generation)
    }
}

pub fn unique_bus_name(name: &UniqueName<'_>) -> BusName<'static> {
    BusName::Unique(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::address;

    fn props(id: &str) -> ResolvedProps {
        ResolvedProps {
            id: id.to_owned(),
            ..ResolvedProps::default()
        }
    }

    fn ids(snapshot: &TraySnapshot) -> Vec<String> {
        snapshot
            .items
            .iter()
            .map(|item| item.key.to_string())
            .collect()
    }

    #[test]
    fn an_item_becomes_visible_only_once_resolved() {
        let mut registry = Registry::default();
        let (seq, generation) = registry.discover(address("org.example.A", ":1.1")).unwrap();
        assert!(registry.snapshot(WatcherState::Connected).items.is_empty());

        assert_eq!(
            registry.apply_resolved(seq, generation, props("a")),
            Applied::Changed
        );
        assert_eq!(ids(&registry.snapshot(WatcherState::Connected)), ["a"]);
    }

    #[test]
    fn re_announcing_the_same_address_is_ignored() {
        let mut registry = Registry::default();
        let addr = address("org.example.A", ":1.1");
        assert!(registry.discover(addr.clone()).is_some());
        assert!(registry.discover(addr).is_none());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn losing_the_owner_removes_the_item_immediately() {
        let mut registry = Registry::default();
        let (seq, generation) = registry.discover(address("org.example.A", ":1.1")).unwrap();
        registry.apply_resolved(seq, generation, props("a"));

        let owner = UniqueName::try_from(":1.1").unwrap();
        assert_eq!(
            registry.remove_for_lost_name(&unique_bus_name(&owner)),
            [seq]
        );
        assert!(registry.snapshot(WatcherState::Connected).items.is_empty());
        assert!(registry.is_empty());
    }

    #[test]
    fn a_reply_from_a_previous_instance_never_touches_its_successor() {
        let mut registry = Registry::default();

        let (seq_a, gen_a) = registry.discover(address("org.example.A", ":1.1")).unwrap();
        registry.apply_resolved(seq_a, gen_a, props("first"));

        let owner = UniqueName::try_from(":1.1").unwrap();
        registry.remove_for_lost_name(&unique_bus_name(&owner));
        let (seq_b, gen_b) = registry.discover(address("org.example.A", ":1.2")).unwrap();
        registry.apply_resolved(seq_b, gen_b, props("second"));

        assert_eq!(
            registry.apply_resolved(seq_a, gen_a, props("first")),
            Applied::Unknown
        );
        assert_eq!(
            registry.apply_resolved(seq_b, gen_a, props("first")),
            Applied::Stale
        );
        assert_eq!(ids(&registry.snapshot(WatcherState::Connected)), ["second"]);
    }

    #[test]
    fn a_refresh_supersedes_the_resolve_it_interrupts() {
        let mut registry = Registry::default();
        let (seq, first) = registry.discover(address("org.example.A", ":1.1")).unwrap();
        registry.apply_resolved(seq, first, props("a"));

        let second = registry.begin_refresh(seq).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            registry.apply_resolved(seq, first, props("stale")),
            Applied::Stale
        );
        assert_eq!(
            registry.apply_resolved(seq, second, props("fresh")),
            Applied::Changed
        );
        assert_eq!(ids(&registry.snapshot(WatcherState::Connected)), ["fresh"]);
    }

    #[test]
    fn a_failing_item_stays_visible_and_does_not_hide_the_others() {
        let mut registry = Registry::default();
        let (broken, broken_gen) = registry
            .discover(address("org.example.Broken", ":1.1"))
            .unwrap();
        let (ok, ok_gen) = registry
            .discover(address("org.example.Ok", ":1.2"))
            .unwrap();

        registry.apply_failure(broken, broken_gen, "timed out".into());
        registry.apply_resolved(ok, ok_gen, props("ok"));

        let snapshot = registry.snapshot(WatcherState::Connected);
        assert_eq!(snapshot.items.len(), 2);
        assert!(matches!(
            snapshot.items[0].state,
            LifecycleState::Degraded { .. }
        ));
        assert_eq!(snapshot.items[1].key.id, "ok");
    }

    #[test]
    fn ordering_is_identical_regardless_of_resolve_order() {
        let addresses = [
            address("org.example.A", ":1.1"),
            address("org.example.B", ":1.2"),
            address("org.example.C", ":1.3"),
            address("org.example.D", ":1.4"),
        ];

        let build = |resolve_order: [usize; 4]| {
            let mut registry = Registry::default();
            let slots: Vec<_> = addresses
                .iter()
                .map(|addr| registry.discover(addr.clone()).unwrap())
                .collect();
            for index in resolve_order {
                let (seq, generation) = slots[index];
                let id = ["a", "b", "c", "d"][index];
                registry.apply_resolved(seq, generation, props(id));
            }
            ids(&registry.snapshot(WatcherState::Connected))
        };

        assert_eq!(build([0, 1, 2, 3]), ["a", "b", "c", "d"]);
        assert_eq!(build([3, 2, 1, 0]), ["a", "b", "c", "d"]);
        assert_eq!(build([2, 0, 3, 1]), ["a", "b", "c", "d"]);
    }

    #[test]
    fn duplicate_ids_are_numbered_by_discovery_order() {
        let mut registry = Registry::default();
        let (first, first_gen) = registry.discover(address("org.example.A", ":1.1")).unwrap();
        let (second, second_gen) = registry.discover(address("org.example.B", ":1.2")).unwrap();

        registry.apply_resolved(second, second_gen, props("chat"));
        registry.apply_resolved(first, first_gen, props("chat"));

        assert_eq!(
            ids(&registry.snapshot(WatcherState::Connected)),
            ["chat", "chat#1"]
        );
    }

    #[test]
    fn reconcile_drops_items_the_watcher_no_longer_lists() {
        let mut registry = Registry::default();
        let kept = address("org.example.A", ":1.1");
        let (seq_a, gen_a) = registry.discover(kept.clone()).unwrap();
        let (seq_b, gen_b) = registry.discover(address("org.example.B", ":1.2")).unwrap();
        registry.apply_resolved(seq_a, gen_a, props("a"));
        registry.apply_resolved(seq_b, gen_b, props("b"));

        assert_eq!(registry.retain_addresses(&[kept]), [seq_b]);
        assert_eq!(ids(&registry.snapshot(WatcherState::Connected)), ["a"]);
    }
}
