mod common;

use std::sync::Arc;

use common::wait_for;
use cosmic_ext_applet_status_hub::core::menu::{EntryKind, MenuModel};
use cosmic_ext_applet_status_hub::core::model::ItemAddress;
use cosmic_ext_applet_status_hub::core::{self, CoreCommand, CoreHandle, MemoryOrderStore};
use cosmic_ext_applet_status_hub::testkit::{
    FakeMenu, FakeWatcher, ItemBehaviour, ItemHandle, MenuBehaviour, PrivateBus,
};
use tokio::sync::watch;

type Menus = watch::Receiver<Option<Arc<MenuModel>>>;

async fn wait_menu(
    menus: &mut Menus,
    what: &str,
    mut predicate: impl FnMut(Option<&Arc<MenuModel>>) -> bool,
) -> Option<Arc<MenuModel>> {
    let deadline = tokio::time::Instant::now() + common::SETTLE;
    loop {
        {
            let current = menus.borrow_and_update().clone();
            if predicate(current.as_ref()) {
                return current;
            }
        }
        assert!(
            tokio::time::timeout_at(deadline, menus.changed())
                .await
                .is_ok(),
            "timed out waiting for {what}"
        );
    }
}

fn labels(model: &MenuModel) -> Vec<&str> {
    model
        .entries
        .iter()
        .filter(|entry| entry.kind != EntryKind::Separator)
        .map(|entry| entry.label.as_str())
        .collect()
}

struct Fixture {
    bus: PrivateBus,
    item_connection: zbus::Connection,
    item: ItemHandle,
    menu: FakeMenu,
    handle: CoreHandle,
    address: ItemAddress,
    snapshots: common::Snapshots,
    menus: Menus,
    #[allow(dead_code)]
    core_task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start(behaviour: MenuBehaviour) -> Self {
        let bus = PrivateBus::start().unwrap();
        let watcher_connection = bus.connect().await.unwrap();
        FakeWatcher::serve(&watcher_connection).await.unwrap();
        std::mem::forget(watcher_connection);

        let item_connection = bus.connect().await.unwrap();
        let menu = FakeMenu::serve(&item_connection, behaviour).await.unwrap();
        let item = ItemHandle::publish(&item_connection, "app", ItemBehaviour::Normal, None)
            .await
            .unwrap();

        let (handle, core_task) = core::spawn_on(
            &tokio::runtime::Handle::current(),
            MemoryOrderStore::default(),
            bus.connect().await.unwrap(),
        );
        let mut snapshots = handle.subscribe();
        let menus = handle.subscribe_menu();

        let snapshot = wait_for(&mut snapshots, "the item", |s| s.items.len() == 1).await;
        let address = snapshot.items[0].address.clone();

        Self {
            bus,
            item_connection,
            item,
            menu,
            handle,
            address,
            snapshots,
            menus,
            core_task,
        }
    }

    fn open(&self) {
        self.handle.send(CoreCommand::Context {
            address: self.address.clone(),
        });
    }
}

#[tokio::test]
async fn a_right_click_opens_the_item_menu() {
    let mut fixture = Fixture::start(MenuBehaviour::Normal).await;
    fixture.open();

    let model = wait_menu(&mut fixture.menus, "the menu", |menu| menu.is_some())
        .await
        .unwrap();

    assert_eq!(labels(&model), ["Open", "Quit"]);
    assert_eq!(model.owner, fixture.address);
    assert_eq!(
        model.entries[1].kind,
        EntryKind::Separator,
        "separators are kept for the view to draw"
    );

    assert_eq!(fixture.item.context_calls().await, 0);
    common::wait_until("the menu announcement", || {
        futures::executor::block_on(async {
            fixture.menu.about_to_show_calls().await >= 1
                && fixture
                    .menu
                    .events()
                    .await
                    .contains(&(0, "opened".to_owned()))
        })
    })
    .await;
}

#[tokio::test]
async fn a_slow_menu_announcement_does_not_delay_the_first_layout() {
    let mut fixture = Fixture::start(MenuBehaviour::SlowAnnouncement).await;
    fixture.open();

    let model = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        wait_menu(&mut fixture.menus, "the first menu layout", |menu| {
            menu.is_some()
        }),
    )
    .await
    .expect("GetLayout must not wait for AboutToShow")
    .unwrap();

    assert_eq!(labels(&model), ["Open", "Quit"]);
}

#[tokio::test]
async fn a_property_of_an_unexpected_type_does_not_lose_the_menu() {
    let mut fixture = Fixture::start(MenuBehaviour::MalformedProperty).await;
    fixture.open();

    let model = wait_menu(&mut fixture.menus, "the menu", |menu| menu.is_some())
        .await
        .unwrap();
    assert_eq!(labels(&model), ["Open", "Quit"]);
}

#[tokio::test]
async fn choosing_an_entry_reports_it_and_closes_the_menu() {
    let mut fixture = Fixture::start(MenuBehaviour::Normal).await;
    fixture.open();
    let model = wait_menu(&mut fixture.menus, "the menu", |menu| menu.is_some())
        .await
        .unwrap();

    let quit = model
        .entries
        .iter()
        .find(|entry| entry.label == "Quit")
        .unwrap();
    fixture.handle.send(CoreCommand::MenuClicked {
        address: model.owner.clone(),
        id: quit.id,
        token: Some("menu-token".to_owned()),
        close: true,
    });

    wait_menu(&mut fixture.menus, "the menu to close", |menu| {
        menu.is_none()
    })
    .await;

    common::wait_until("the click to reach the application", || {
        futures::executor::block_on(fixture.menu.events())
            .contains(&(quit.id, "clicked".to_owned()))
    })
    .await;
    assert!(
        fixture
            .menu
            .events()
            .await
            .contains(&(0, "closed".to_owned()))
    );
    assert_eq!(fixture.item.activation_tokens().await, ["menu-token"]);
    assert!(
        fixture
            .menu
            .event_details()
            .await
            .iter()
            .all(|event| event.timestamp == 0)
    );
}

#[tokio::test]
async fn choosing_a_submenu_reports_it_without_closing_the_menu() {
    let mut fixture = Fixture::start(MenuBehaviour::Submenu).await;
    fixture.open();
    let model = wait_menu(&mut fixture.menus, "the submenu", |menu| menu.is_some())
        .await
        .unwrap();
    let submenu = &model.entries[0];

    fixture.handle.send(CoreCommand::MenuClicked {
        address: model.owner.clone(),
        id: submenu.id,
        token: Some("submenu-token".to_owned()),
        close: false,
    });

    common::wait_until("the submenu click to reach the application", || {
        futures::executor::block_on(fixture.menu.events())
            .contains(&(submenu.id, "clicked".to_owned()))
    })
    .await;
    assert!(fixture.menus.borrow_and_update().is_some());
    assert_eq!(fixture.item.activation_tokens().await, ["submenu-token"]);
    assert!(
        !fixture
            .menu
            .events()
            .await
            .contains(&(0, "closed".to_owned()))
    );
}

#[tokio::test]
async fn an_application_changing_its_menu_updates_what_is_shown() {
    let mut fixture = Fixture::start(MenuBehaviour::Normal).await;
    fixture.open();
    let first = wait_menu(&mut fixture.menus, "the menu", |menu| menu.is_some())
        .await
        .unwrap();
    assert_eq!(labels(&first), ["Open", "Quit"]);

    fixture
        .menu
        .announce_change(&fixture.item_connection, MenuBehaviour::MalformedProperty)
        .await
        .unwrap();

    let updated = wait_menu(&mut fixture.menus, "the updated menu", |menu| {
        menu.is_some_and(|model| model.revision > first.revision)
    })
    .await
    .unwrap();
    assert_eq!(labels(&updated), ["Open", "Quit"]);
    assert!(updated.revision > first.revision);
}

#[tokio::test]
async fn an_application_dying_with_its_menu_open_closes_it_safely() {
    let mut fixture = Fixture::start(MenuBehaviour::Normal).await;
    fixture.open();
    wait_menu(&mut fixture.menus, "the menu", |menu| menu.is_some()).await;

    let survivor = fixture.bus.connect().await.unwrap();
    ItemHandle::publish(&survivor, "survivor", ItemBehaviour::Normal, None)
        .await
        .unwrap();
    wait_for(&mut fixture.snapshots, "both items", |s| s.items.len() == 2).await;

    drop(fixture.item);
    drop(fixture.item_connection);

    wait_menu(&mut fixture.menus, "the menu to be invalidated", |menu| {
        menu.is_none()
    })
    .await;
    let remaining = wait_for(&mut fixture.snapshots, "one item left", |s| {
        s.items.len() == 1
    })
    .await;
    assert_eq!(remaining.items[0].key.id, "survivor");
}

#[tokio::test]
async fn a_menu_that_cannot_be_fetched_opens_nothing() {
    let fixture = Fixture::start(MenuBehaviour::Broken).await;
    fixture.open();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        fixture.menus.borrow().is_none(),
        "a menu that failed to load must not be presented"
    );
}

#[tokio::test]
async fn a_menu_with_nothing_in_it_opens_nothing() {
    let fixture = Fixture::start(MenuBehaviour::Empty).await;
    fixture.open();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        fixture.menus.borrow().is_none(),
        "a menu of separators has nothing to show"
    );
}

#[tokio::test]
async fn a_left_click_never_falls_back_to_the_menu() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let item_connection = bus.connect().await.unwrap();
    FakeMenu::serve(&item_connection, MenuBehaviour::Normal)
        .await
        .unwrap();
    let item = ItemHandle::publish(
        &item_connection,
        "menu-only",
        ItemBehaviour::NoPrimaryAction,
        None,
    )
    .await
    .unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();
    let mut menus = handle.subscribe_menu();

    let snapshot = wait_for(&mut snapshots, "the item", |s| s.items.len() == 1).await;
    handle.send(CoreCommand::Primary {
        address: snapshot.items[0].address.clone(),
        token: None,
    });

    common::wait_until("the activation attempt", || {
        futures::executor::block_on(item.activate_calls()) == 1
    })
    .await;
    assert!(menus.borrow_and_update().is_none());

    handle.send(CoreCommand::Context {
        address: snapshot.items[0].address.clone(),
    });
    wait_menu(&mut menus, "the menu of a right click", |menu| {
        menu.is_some()
    })
    .await;
    assert_eq!(
        item.activate_calls().await,
        1,
        "a right click must not activate the item"
    );
}

#[tokio::test]
async fn a_left_click_activates_even_a_menu_only_item() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let item_connection = bus.connect().await.unwrap();
    let item = ItemHandle::publish(
        &item_connection,
        "item-is-menu",
        ItemBehaviour::ItemIsMenu,
        None,
    )
    .await
    .unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();
    let mut menus = handle.subscribe_menu();
    let snapshot = wait_for(&mut snapshots, "the item", |s| s.items.len() == 1).await;

    handle.send(CoreCommand::Primary {
        address: snapshot.items[0].address.clone(),
        token: Some("primary-token".to_owned()),
    });

    common::wait_until("the primary action", || {
        futures::executor::block_on(item.activate_calls()) == 1
    })
    .await;
    assert_eq!(item.activation_tokens().await, ["primary-token"]);
    assert!(menus.borrow_and_update().is_none());
}

#[tokio::test]
async fn a_right_click_without_a_dbus_menu_calls_the_remote_context_menu() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let item_connection = bus.connect().await.unwrap();
    let item = ItemHandle::publish(&item_connection, "no-menu", ItemBehaviour::NoMenu, None)
        .await
        .unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();
    let mut menus = handle.subscribe_menu();
    let snapshot = wait_for(&mut snapshots, "the item", |s| s.items.len() == 1).await;

    handle.send(CoreCommand::Context {
        address: snapshot.items[0].address.clone(),
    });

    common::wait_until("the context menu request", || {
        futures::executor::block_on(item.context_calls()) == 1
    })
    .await;
    assert!(menus.borrow_and_update().is_none());
    assert_eq!(item.context_calls().await, 1);
}

#[tokio::test]
async fn a_middle_click_requests_the_secondary_action() {
    let bus = PrivateBus::start().unwrap();
    let watcher_connection = bus.connect().await.unwrap();
    FakeWatcher::serve(&watcher_connection).await.unwrap();

    let item_connection = bus.connect().await.unwrap();
    let item = ItemHandle::publish(&item_connection, "secondary", ItemBehaviour::NoMenu, None)
        .await
        .unwrap();

    let (handle, _join) = core::spawn_on(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
        bus.connect().await.unwrap(),
    );
    let mut snapshots = handle.subscribe();
    let snapshot = wait_for(&mut snapshots, "the item", |s| s.items.len() == 1).await;

    handle.send(CoreCommand::Secondary {
        address: snapshot.items[0].address.clone(),
    });

    common::wait_until("the secondary action", || {
        futures::executor::block_on(item.secondary_calls()) == 1
    })
    .await;
}
