mod common;

use std::sync::{Arc, Mutex};

use common::{keys, wait_for, wait_until};
use cosmic_status_hub::core::model::ItemKey;
use cosmic_status_hub::core::{self, MemoryOrderStore, OrderStore};
use cosmic_status_hub::testkit::{FakeWatcher, ItemBehaviour, ItemHandle, PrivateBus};

#[derive(Clone, Debug, Default)]
struct SharedOrder(Arc<Mutex<Vec<ItemKey>>>);

impl OrderStore for SharedOrder {
    fn load(&self) -> Vec<ItemKey> {
        self.0.lock().unwrap().clone()
    }

    fn store(&mut self, order: &[ItemKey]) {
        *self.0.lock().unwrap() = order.to_vec();
    }
}

#[tokio::test]
async fn two_independent_hosts_agree_on_the_order() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let runtime = tokio::runtime::Handle::current();
    let order = SharedOrder::default();

    let (monitor_a, _a) = core::spawn_on(&runtime, order.clone(), bus.connect().await.unwrap());
    let (monitor_b, _b) = core::spawn_on(&runtime, order.clone(), bus.connect().await.unwrap());
    let mut view_a = monitor_a.subscribe();
    let mut view_b = monitor_b.subscribe();

    let mut connections = Vec::new();
    for id in ["steam", "discord", "nextcloud", "telegram"] {
        let connection = bus.connect().await.unwrap();
        ItemHandle::publish(&connection, id, ItemBehaviour::Normal, None)
            .await
            .unwrap();
        connections.push(connection);
    }

    let a = wait_for(&mut view_a, "four items on monitor A", |s| {
        s.items.len() == 4
    })
    .await;
    let b = wait_for(&mut view_b, "four items on monitor B", |s| {
        s.items.len() == 4
    })
    .await;

    assert_eq!(keys(&a), ["steam", "discord", "nextcloud", "telegram"]);
    assert_eq!(keys(&a), keys(&b));

    drop(connections.remove(1));
    let a = wait_for(&mut view_a, "three items on monitor A", |s| {
        s.items.len() == 3
    })
    .await;
    let b = wait_for(&mut view_b, "three items on monitor B", |s| {
        s.items.len() == 3
    })
    .await;
    assert_eq!(keys(&a), ["steam", "nextcloud", "telegram"]);
    assert_eq!(keys(&a), keys(&b));
}

#[tokio::test]
async fn two_hosts_recover_an_icon_published_late_without_a_signal() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let item_connection = bus.connect().await.unwrap();
    let item = ItemHandle::publish(&item_connection, "late-icon", ItemBehaviour::Normal, None)
        .await
        .unwrap();
    item.set_icon_name_silently("").await;

    let runtime = tokio::runtime::Handle::current();
    let (monitor_a, _a) = core::spawn_on(
        &runtime,
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let (monitor_b, _b) = core::spawn_on(
        &runtime,
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut view_a = monitor_a.subscribe();
    let mut view_b = monitor_b.subscribe();

    let initial_a = wait_for(&mut view_a, "empty icon on monitor A", |snapshot| {
        snapshot
            .items
            .first()
            .is_some_and(|item| item.icon.icon_name.is_empty())
    })
    .await;
    let initial_b = wait_for(&mut view_b, "empty icon on monitor B", |snapshot| {
        snapshot
            .items
            .first()
            .is_some_and(|item| item.icon.icon_name.is_empty())
    })
    .await;

    item.set_icon_name_silently("application-default").await;

    let settled_a = wait_for(&mut view_a, "settled icon on monitor A", |snapshot| {
        snapshot
            .items
            .first()
            .is_some_and(|item| item.icon.icon_name == "application-default")
    })
    .await;
    let settled_b = wait_for(&mut view_b, "settled icon on monitor B", |snapshot| {
        snapshot
            .items
            .first()
            .is_some_and(|item| item.icon.icon_name == "application-default")
    })
    .await;

    assert!(settled_a.items[0].generation > initial_a.items[0].generation);
    assert!(settled_b.items[0].generation > initial_b.items[0].generation);
}

#[tokio::test]
async fn a_second_subscriber_sees_the_same_order_as_the_first() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut first = handle.subscribe();

    let mut connections = Vec::new();
    for id in ["a", "b", "c", "d"] {
        let connection = bus.connect().await.unwrap();
        ItemHandle::publish(&connection, id, ItemBehaviour::Normal, None)
            .await
            .unwrap();
        connections.push(connection);
    }
    let expected = wait_for(&mut first, "four items", |s| s.items.len() == 4).await;
    assert_eq!(keys(&expected), ["a", "b", "c", "d"]);

    let mut late = handle.subscribe();
    assert_eq!(keys(&late.borrow_and_update()), keys(&expected));
}

#[tokio::test]
async fn a_restarted_host_rebuilds_the_same_order() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let runtime = tokio::runtime::Handle::current();
    let order = SharedOrder::default();

    let (first_run, first_join) =
        core::spawn_on(&runtime, order.clone(), bus.connect().await.unwrap());
    let mut view = first_run.subscribe();

    let mut connections = Vec::new();
    for id in ["steam", "discord", "telegram", "nextcloud"] {
        let connection = bus.connect().await.unwrap();
        ItemHandle::publish(&connection, id, ItemBehaviour::Normal, None)
            .await
            .unwrap();
        connections.push(connection);
    }
    let before = wait_for(&mut view, "four items", |s| s.items.len() == 4).await;
    assert_eq!(keys(&before), ["steam", "discord", "telegram", "nextcloud"]);

    wait_until("the order to be persisted", || order.load().len() == 4).await;

    drop(first_run);
    let _ = first_join.await;

    let (second_run, _second_join) =
        core::spawn_on(&runtime, order.clone(), bus.connect().await.unwrap());
    let mut view = second_run.subscribe();
    let after = wait_for(&mut view, "the rebuilt tray", |s| s.items.len() == 4).await;

    assert_eq!(keys(&after), keys(&before));
}

#[tokio::test]
async fn remembered_order_survives_applications_restarting_out_of_order() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let runtime = tokio::runtime::Handle::current();
    let order = SharedOrder::default();

    let (first_run, first_join) =
        core::spawn_on(&runtime, order.clone(), bus.connect().await.unwrap());
    let mut view = first_run.subscribe();

    let mut connections = Vec::new();
    for id in ["steam", "discord", "telegram"] {
        let connection = bus.connect().await.unwrap();
        ItemHandle::publish(&connection, id, ItemBehaviour::Normal, None)
            .await
            .unwrap();
        connections.push(connection);
    }
    wait_for(&mut view, "three items", |s| s.items.len() == 3).await;
    wait_until("the order to be persisted", || order.load().len() == 3).await;

    drop(first_run);
    let _ = first_join.await;
    connections.clear();

    let mut connections = Vec::new();
    for id in ["telegram", "discord", "steam"] {
        let connection = bus.connect().await.unwrap();
        ItemHandle::publish(&connection, id, ItemBehaviour::Normal, None)
            .await
            .unwrap();
        connections.push(connection);
    }

    let (second_run, _second_join) =
        core::spawn_on(&runtime, order.clone(), bus.connect().await.unwrap());
    let mut view = second_run.subscribe();
    let after = wait_for(&mut view, "the rebuilt tray", |s| s.items.len() == 3).await;

    assert_eq!(keys(&after), ["steam", "discord", "telegram"]);
}

#[tokio::test]
async fn the_tray_survives_the_watcher_disappearing_and_returning() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut view = handle.subscribe();

    let steam = bus.connect().await.unwrap();
    ItemHandle::publish(&steam, "steam", ItemBehaviour::Normal, None)
        .await
        .unwrap();
    wait_for(&mut view, "the item", |s| s.items.len() == 1).await;

    drop(watcher_connection);
    let degraded = wait_for(&mut view, "the watcher to be reported missing", |s| {
        matches!(s.watcher, core::model::WatcherState::Unavailable(_))
    })
    .await;
    assert_eq!(
        keys(&degraded),
        ["steam"],
        "items must not vanish with the watcher"
    );

    let replacement = bus.connect().await.unwrap();
    let watcher = FakeWatcher::serve(&replacement).await.unwrap();
    let steam_again = bus.connect().await.unwrap();
    ItemHandle::publish(&steam_again, "steam", ItemBehaviour::Normal, None)
        .await
        .unwrap();
    drop(steam);

    let recovered = wait_for(&mut view, "the tray to recover", |s| {
        s.watcher == core::model::WatcherState::Connected && s.items.len() == 1
    })
    .await;
    assert_eq!(keys(&recovered), ["steam"]);
    assert_eq!(
        watcher.hosts().await.len(),
        1,
        "the host re-registers after reconnecting"
    );
}
