mod common;

use std::time::Duration;

use common::{keys, wait_for};
use cosmic_status_hub::core::{self, MemoryOrderStore};
use cosmic_status_hub::testkit::{
    FakeCosmicWatcher, FakeWatcher, ItemBehaviour, ItemHandle, PrivateBus,
};

#[tokio::test]
async fn rebuilds_state_from_a_watcher_that_was_already_populated() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let steam = bus.connect().await.unwrap();
    ItemHandle::publish(&steam, "steam", ItemBehaviour::Normal, None)
        .await
        .unwrap();
    let discord = bus.connect().await.unwrap();
    ItemHandle::publish(&discord, "discord", ItemBehaviour::Normal, None)
        .await
        .unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();

    let snapshot = wait_for(&mut snapshots, "both pre-existing items", |s| {
        s.items.len() == 2
    })
    .await;
    assert_eq!(keys(&snapshot), ["steam", "discord"]);
}

#[tokio::test]
async fn accepts_object_path_and_bus_name_registrations() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();
    wait_for(&mut snapshots, "the host to attach", |s| {
        s.watcher == core::model::WatcherState::Connected
    })
    .await;

    let kde = bus.connect().await.unwrap();
    ItemHandle::publish(
        &kde,
        "kde-style",
        ItemBehaviour::Normal,
        Some("org.kde.StatusNotifierItem-4242-1"),
    )
    .await
    .unwrap();

    let path_style = bus.connect().await.unwrap();
    ItemHandle::publish(&path_style, "path-style", ItemBehaviour::Normal, None)
        .await
        .unwrap();

    let snapshot = wait_for(&mut snapshots, "both registration styles", |s| {
        s.items.len() == 2
    })
    .await;
    assert_eq!(keys(&snapshot), ["kde-style", "path-style"]);
}

#[tokio::test]
async fn losing_the_owner_removes_the_item() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();

    let doomed = bus.connect().await.unwrap();
    ItemHandle::publish(&doomed, "doomed", ItemBehaviour::Normal, None)
        .await
        .unwrap();
    let survivor = bus.connect().await.unwrap();
    ItemHandle::publish(&survivor, "survivor", ItemBehaviour::Normal, None)
        .await
        .unwrap();

    wait_for(&mut snapshots, "both items", |s| s.items.len() == 2).await;

    drop(doomed);

    let snapshot = wait_for(&mut snapshots, "the dead item to disappear", |s| {
        s.items.len() == 1
    })
    .await;
    assert_eq!(keys(&snapshot), ["survivor"]);
}

#[tokio::test]
async fn a_restarted_application_replaces_its_previous_instance() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();

    let first = bus.connect().await.unwrap();
    ItemHandle::publish(&first, "chat", ItemBehaviour::Normal, None)
        .await
        .unwrap();
    let before = wait_for(&mut snapshots, "the first instance", |s| s.items.len() == 1).await;
    let first_owner = before.items[0].address.owner.clone();

    drop(first);
    wait_for(&mut snapshots, "the first instance to go", |s| {
        s.items.is_empty()
    })
    .await;

    let second = bus.connect().await.unwrap();
    ItemHandle::publish(&second, "chat", ItemBehaviour::Normal, None)
        .await
        .unwrap();
    let after = wait_for(&mut snapshots, "the second instance", |s| {
        s.items.len() == 1
    })
    .await;

    assert_eq!(keys(&after), ["chat"]);
    assert_ne!(
        after.items[0].address.owner, first_owner,
        "the new item must be bound to the new owner"
    );
    assert!(after.items[0].generation > before.items[0].generation);
}

#[tokio::test]
async fn a_change_signal_refreshes_the_item() {
    common::init();
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();

    let connection = bus.connect().await.unwrap();
    let item = ItemHandle::publish(&connection, "app", ItemBehaviour::Normal, None)
        .await
        .unwrap();
    wait_for(&mut snapshots, "the item", |s| s.items.len() == 1).await;

    item.set_title("Renamed").await.unwrap();

    let snapshot = wait_for(&mut snapshots, "the new title", |s| {
        s.items.first().is_some_and(|item| item.title == "Renamed")
    })
    .await;
    assert_eq!(keys(&snapshot), ["app"]);
}

#[tokio::test]
async fn a_hanging_item_is_isolated_from_the_rest() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();

    let stuck = bus.connect().await.unwrap();
    ItemHandle::publish(&stuck, "stuck", ItemBehaviour::Hangs, None)
        .await
        .unwrap();
    let healthy = bus.connect().await.unwrap();
    ItemHandle::publish(&healthy, "healthy", ItemBehaviour::Normal, None)
        .await
        .unwrap();

    let start = std::time::Instant::now();
    wait_for(&mut snapshots, "the healthy item", |s| {
        s.items.iter().any(|item| item.key.id == "healthy")
    })
    .await;
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "a wedged application delayed an unrelated one"
    );

    let snapshot = wait_for(&mut snapshots, "the stuck item to degrade", |s| {
        s.items.len() == 2
    })
    .await;
    let stuck_item = snapshot
        .items
        .iter()
        .find(|item| item.address.owner == *stuck.unique_name().unwrap())
        .unwrap();
    assert!(matches!(
        stuck_item.state,
        core::lifecycle::LifecycleState::Degraded { .. }
    ));
}

#[tokio::test]
async fn a_broken_item_degrades_without_disappearing() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();

    let broken = bus.connect().await.unwrap();
    ItemHandle::publish(&broken, "broken", ItemBehaviour::Broken, None)
        .await
        .unwrap();
    let fine = bus.connect().await.unwrap();
    ItemHandle::publish(&fine, "fine", ItemBehaviour::Normal, None)
        .await
        .unwrap();

    let snapshot = wait_for(&mut snapshots, "both items", |s| s.items.len() == 2).await;
    assert!(
        snapshot
            .items
            .iter()
            .any(|item| matches!(item.state, core::lifecycle::LifecycleState::Degraded { .. })),
        "the broken item should be degraded, not missing"
    );
    assert!(snapshot.items.iter().any(|item| item.key.id == "fine"));
}

#[tokio::test]
async fn the_host_registers_itself_with_the_watcher() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    let watcher = FakeWatcher::serve(&watcher_connection).await.unwrap();

    let client = bus.connect().await.unwrap();
    let expected_name = client.unique_name().unwrap().to_string();
    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        client,
    );
    let mut snapshots = handle.subscribe();
    wait_for(&mut snapshots, "the host to attach", |s| {
        s.watcher == core::model::WatcherState::Connected
    })
    .await;

    let hosts = watcher.hosts().await;
    assert_eq!(
        hosts.len(),
        1,
        "expected exactly one host registration, got {hosts:?}"
    );
    assert_eq!(hosts[0], expected_name);
}

#[tokio::test]
async fn every_host_keeps_the_cosmic_watcher_alive() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();
    let cosmic = FakeCosmicWatcher::serve(&watcher_connection).await.unwrap();

    let runtime = tokio::runtime::Handle::current();
    let (first, _first_join) = core::spawn_on(
        &runtime,
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let (second, _second_join) = core::spawn_on(
        &runtime,
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut first_snapshots = first.subscribe();
    let mut second_snapshots = second.subscribe();
    wait_for(
        &mut first_snapshots,
        "the first host to attach",
        |snapshot| snapshot.watcher == core::model::WatcherState::Connected,
    )
    .await;
    wait_for(
        &mut second_snapshots,
        "the second host to attach",
        |snapshot| snapshot.watcher == core::model::WatcherState::Connected,
    )
    .await;

    assert_eq!(cosmic.clients().await.len(), 2);
}

#[tokio::test]
async fn an_item_that_starts_answering_again_needs_no_signal_to_come_back() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();

    let connection = bus.connect().await.unwrap();
    let item = ItemHandle::publish(&connection, "late", ItemBehaviour::Broken, None)
        .await
        .unwrap();

    let snapshot = wait_for(&mut snapshots, "the item to degrade", |s| {
        s.items
            .iter()
            .any(|item| matches!(item.state, core::lifecycle::LifecycleState::Degraded { .. }))
    })
    .await;
    let degraded = &snapshot.items[0];
    assert_ne!(degraded.key.id, "late");
    assert!(degraded.menu_path.is_none());

    item.set_behaviour(ItemBehaviour::Normal).await;

    let snapshot = wait_for(&mut snapshots, "the item to resolve itself", |s| {
        s.items.iter().any(|item| item.key.id == "late")
    })
    .await;
    let recovered = &snapshot.items[0];
    assert!(recovered.state.is_resolved());
    assert!(recovered.menu_path.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_item_that_answers_only_some_properties_is_asked_again() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();

    let connection = bus.connect().await.unwrap();
    let item = ItemHandle::publish(&connection, "half", ItemBehaviour::PartlyStalls, None)
        .await
        .unwrap();

    let snapshot = wait_for(&mut snapshots, "the partial answer to arrive", |s| {
        s.items
            .iter()
            .any(|item| item.icon.icon_name == "application-default")
    })
    .await;
    let partial = &snapshot.items[0];
    assert!(
        partial.menu_path.is_none(),
        "the menu timed out, so a right click has nothing to open"
    );

    item.set_behaviour(ItemBehaviour::Normal).await;

    let snapshot = wait_for(
        &mut snapshots,
        "the rest of the properties to arrive",
        |s| s.items.iter().any(|item| item.menu_path.is_some()),
    )
    .await;
    let complete = &snapshot.items[0];
    assert!(complete.state.is_resolved());
    assert_eq!(complete.key.id, "half");
}
