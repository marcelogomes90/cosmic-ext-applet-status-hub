use zbus::names::{BusName, OwnedUniqueName, UniqueName};
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use crate::core::lifecycle::LifecycleState;
use crate::core::model::{
    Category, DEFAULT_ITEM_PATH, DiscoverySeq, Generation, IconSource, ItemAddress, ItemKey,
    ItemStatus, TrayItem,
};

pub fn address(service: &str, owner: &str) -> ItemAddress {
    ItemAddress {
        service: BusName::try_from(service.to_owned()).unwrap().into(),
        path: OwnedObjectPath::from(ObjectPath::try_from(DEFAULT_ITEM_PATH).unwrap()),
        owner: OwnedUniqueName::from(UniqueName::try_from(owner.to_owned()).unwrap()),
    }
}

pub fn item(id: &str, seq: u64) -> TrayItem {
    TrayItem {
        address: address("org.example.Item", &format!(":1.{seq}")),
        key: ItemKey::new(id, 0),
        generation: Generation(seq),
        discovery_seq: DiscoverySeq(seq),
        state: LifecycleState::Ready,
        id: id.to_owned(),
        title: id.to_owned(),
        category: Category::ApplicationStatus,
        status: ItemStatus::Active,
        item_is_menu: false,
        menu_path: None,
        tooltip: None,
        icon: std::sync::Arc::new(IconSource::default()),
    }
}
