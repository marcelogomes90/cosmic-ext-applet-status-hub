pub mod call;
pub mod host;
pub mod icons;
pub mod lifecycle;
pub mod menu;
pub mod model;
pub mod ordering;
pub mod proxies;
pub mod registry;

#[cfg(test)]
pub mod testing;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use zbus::names::BusName;

use crate::core::call::{
    ACTION_TIMEOUT, Backoff, CallError, LAYOUT_TIMEOUT, PROPERTY_TIMEOUT, with_timeout,
};
use crate::core::menu::MenuModel;
use crate::core::model::{
    Category, DiscoverySeq, Generation, IconSource, ItemAddress, ItemKey, ItemStatus, TraySnapshot,
    WatcherState,
};
use crate::core::proxies::{DBusMenuProxy, RawLayout, StatusNotifierItemProxy};
use crate::core::registry::{Applied, Registry, ResolvedProps};
use zbus::zvariant::OwnedObjectPath;

const PERSIST_DEBOUNCE: Duration = Duration::from_secs(2);
const RESOLVE_RETRY_DELAYS: [Duration; 4] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(8),
];

pub trait OrderStore: Send + 'static {
    fn load(&self) -> Vec<ItemKey>;
    fn store(&mut self, order: &[ItemKey]);
}

#[derive(Debug, Default)]
pub struct MemoryOrderStore(Vec<ItemKey>);

impl OrderStore for MemoryOrderStore {
    fn load(&self) -> Vec<ItemKey> {
        self.0.clone()
    }

    fn store(&mut self, order: &[ItemKey]) {
        self.0 = order.to_vec();
    }
}

#[derive(Clone, Debug)]
pub enum CoreCommand {
    Primary {
        address: ItemAddress,
        token: Option<String>,
    },
    Secondary {
        address: ItemAddress,
    },
    Context {
        address: ItemAddress,
    },
    CloseMenu,
    MenuClicked {
        address: ItemAddress,
        id: i32,
        token: Option<String>,
        close: bool,
    },
    SetRemembered(Vec<ItemKey>),
}

#[derive(Clone, Debug)]
pub struct CoreHandle {
    snapshots: watch::Receiver<Arc<TraySnapshot>>,
    menu: watch::Receiver<Option<Arc<MenuModel>>>,
    commands: mpsc::Sender<CoreCommand>,
}

impl CoreHandle {
    pub fn snapshot(&self) -> Arc<TraySnapshot> {
        self.snapshots.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<TraySnapshot>> {
        self.snapshots.clone()
    }

    pub fn subscribe_menu(&self) -> watch::Receiver<Option<Arc<MenuModel>>> {
        self.menu.clone()
    }

    pub fn send(&self, command: CoreCommand) {
        if let Err(err) = self.commands.try_send(command) {
            tracing::warn!(error = %err, "dropping command, core is not accepting work");
        }
    }
}

enum Event {
    Registered(String),
    Unregistered(String),
    NameLost(BusName<'static>),
    Resolved {
        seq: DiscoverySeq,
        generation: Generation,
        result: Box<ResolveResult>,
    },
    Changed {
        seq: DiscoverySeq,
    },
    RetryResolve {
        seq: DiscoverySeq,
        generation: Generation,
    },
    Command(CoreCommand),
    MenuLayout {
        token: MenuToken,
        result: Box<Result<(u32, RawLayout), String>>,
    },
    MenuChanged {
        token: MenuToken,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuToken(u64);

type ResolveResult = Result<ResolvedProps, String>;

pub fn spawn(
    runtime: &tokio::runtime::Handle,
    store: impl OrderStore,
) -> (CoreHandle, JoinHandle<()>) {
    spawn_inner(runtime, store, None)
}

pub fn spawn_on(
    runtime: &tokio::runtime::Handle,
    store: impl OrderStore,
    connection: zbus::Connection,
) -> (CoreHandle, JoinHandle<()>) {
    spawn_inner(runtime, store, Some(connection))
}

fn spawn_inner(
    runtime: &tokio::runtime::Handle,
    store: impl OrderStore,
    connection: Option<zbus::Connection>,
) -> (CoreHandle, JoinHandle<()>) {
    let (snapshot_tx, snapshot_rx) = watch::channel(Arc::new(TraySnapshot::default()));
    let (menu_tx, menu_rx) = watch::channel(None);
    let (command_tx, command_rx) = mpsc::channel(64);

    let join = runtime.spawn(async move {
        Core::new(store, snapshot_tx, menu_tx, command_rx)
            .run(connection)
            .await;
    });

    (
        CoreHandle {
            snapshots: snapshot_rx,
            menu: menu_rx,
            commands: command_tx,
        },
        join,
    )
}

struct OpenMenu {
    token: MenuToken,
    address: ItemAddress,
    seq: DiscoverySeq,
    item_generation: Generation,
    menu_path: OwnedObjectPath,
    connection: zbus::Connection,
    watch: JoinHandle<()>,
}

impl OpenMenu {
    async fn proxy(&self) -> zbus::Result<DBusMenuProxy<'static>> {
        menu_proxy(&self.connection, &self.address, self.menu_path.clone()).await
    }
}

struct ItemTasks {
    signals: JoinHandle<()>,
    resolve: Option<JoinHandle<()>>,
    retry: Option<JoinHandle<()>>,
    retry_attempts: usize,
}

impl ItemTasks {
    fn abort(self) {
        self.signals.abort();
        if let Some(resolve) = self.resolve {
            resolve.abort();
        }
        if let Some(retry) = self.retry {
            retry.abort();
        }
    }
}

struct Core<S: OrderStore> {
    registry: Registry,
    store: S,
    snapshots: watch::Sender<Arc<TraySnapshot>>,
    menus: watch::Sender<Option<Arc<MenuModel>>>,
    commands: mpsc::Receiver<CoreCommand>,
    tasks: HashMap<DiscoverySeq, ItemTasks>,
    internal_tx: mpsc::Sender<Event>,
    internal_rx: mpsc::Receiver<Event>,
    watcher: WatcherState,
    open_menu: Option<OpenMenu>,
    next_menu_token: u64,
    to_persist: Vec<ItemKey>,
    persist_at: Option<Instant>,
}

impl<S: OrderStore> Core<S> {
    fn new(
        store: S,
        snapshots: watch::Sender<Arc<TraySnapshot>>,
        menus: watch::Sender<Option<Arc<MenuModel>>>,
        commands: mpsc::Receiver<CoreCommand>,
    ) -> Self {
        let (internal_tx, internal_rx) = mpsc::channel(256);
        let baseline = store.load();
        Self {
            registry: Registry::new(baseline.clone()),
            to_persist: baseline,
            store,
            snapshots,
            menus,
            commands,
            tasks: HashMap::new(),
            internal_tx,
            internal_rx,
            watcher: WatcherState::Connecting,
            open_menu: None,
            next_menu_token: 0,
            persist_at: None,
        }
    }

    async fn run(mut self, connection: Option<zbus::Connection>) {
        let connection = if let Some(connection) = connection {
            connection
        } else {
            let Some(connection) = connect_session().await else {
                self.watcher = WatcherState::Unavailable("no session bus".into());
                self.publish();
                return;
            };
            connection
        };

        let mut lost_names = match host::lost_name_stream(&connection).await {
            Ok(stream) => Box::pin(stream),
            Err(err) => {
                tracing::error!(error = %err, "cannot observe NameOwnerChanged");
                self.watcher = WatcherState::Unavailable(err.to_string());
                self.publish();
                return;
            }
        };

        let (mut registered, mut unregistered) = split(self.attach(&connection).await);
        let mut backoff = Backoff::new(u64::from(std::process::id()));
        let mut retry_at: Option<Instant> = registered
            .is_none()
            .then(|| Instant::now() + backoff.next_delay());

        loop {
            let event = tokio::select! {
                biased;

                Some(name) = lost_names.next() => Event::NameLost(name),

                Some(signal) = next_or_pending(unregistered.as_mut()) => match signal.args() {
                    Ok(args) => Event::Unregistered(args.service.to_owned()),
                    Err(err) => {
                        tracing::debug!(error = %err, "malformed unregister signal");
                        continue;
                    }
                },

                Some(signal) = next_or_pending(registered.as_mut()) => match signal.args() {
                    Ok(args) => Event::Registered(args.service.to_owned()),
                    Err(err) => {
                        tracing::debug!(error = %err, "malformed register signal");
                        continue;
                    }
                },

                Some(event) = self.internal_rx.recv() => event,

                command = self.commands.recv() => match command {
                    Some(command) => Event::Command(command),
                    None => break,
                },

                () = sleep_until_opt(retry_at) => {
                    (registered, unregistered) = split(self.attach(&connection).await);
                    if registered.is_some() {
                        backoff.reset();
                        retry_at = None;
                    } else {
                        retry_at = Some(Instant::now() + backoff.next_delay());
                    }
                    continue;
                }

                () = sleep_until_opt(self.persist_at) => {
                    self.flush_order();
                    continue;
                }
            };

            let watcher_died = matches!(&event, Event::NameLost(name)
                if name.as_str() == host::WATCHER_NAME);

            self.handle(&connection, event).await;

            if watcher_died {
                tracing::warn!("watcher connection lost");
                self.watcher = WatcherState::Unavailable("watcher exited".into());
                registered = None;
                unregistered = None;
                backoff.reset();
                retry_at = Some(Instant::now() + backoff.next_delay());
                self.publish();
            }
        }

        self.flush_order();
        self.close_menu();
        for (_, tasks) in self.tasks.drain() {
            tasks.abort();
        }
    }

    async fn attach(&mut self, connection: &zbus::Connection) -> Option<host::WatcherLink> {
        if let Err(err) = host::try_activate(connection).await {
            tracing::warn!(error = %err, "watcher preparation failed");
            self.watcher = WatcherState::Unavailable(err.to_string());
            self.publish();
            return None;
        }

        let link = match host::connect(connection).await {
            Ok(link) => link,
            Err(err) => {
                tracing::warn!(error = %err, "watcher unavailable");
                self.watcher = WatcherState::Unavailable(err.to_string());
                self.publish();
                return None;
            }
        };

        tracing::info!(items = link.initial.len(), "state rebuilding");
        let mut live = Vec::with_capacity(link.initial.len());
        for entry in &link.initial {
            if let Some(address) = self.introduce(connection, entry).await {
                live.push(address);
            }
        }
        let dropped = self.registry.retain_addresses(&live);
        for seq in &dropped {
            self.drop_tasks(*seq);
        }
        if self
            .open_menu
            .as_ref()
            .is_some_and(|open| dropped.contains(&open.seq))
        {
            self.invalidate_menu("item gone during reconcile");
        }

        self.watcher = WatcherState::Connected;
        tracing::info!(items = self.registry.len(), "registry rebuilt");
        self.publish();
        Some(link)
    }

    async fn handle(&mut self, connection: &zbus::Connection, event: Event) {
        match event {
            Event::Registered(entry) => {
                self.introduce(connection, &entry).await;
            }

            Event::Unregistered(entry) => match model::parse_service_entry(&entry) {
                Ok((service, path)) => {
                    let removed = self.registry.remove_for_service(service.inner(), &path);
                    if !removed.is_empty() {
                        for seq in &removed {
                            self.drop_tasks(*seq);
                        }
                        if self
                            .open_menu
                            .as_ref()
                            .is_some_and(|open| removed.contains(&open.seq))
                        {
                            self.invalidate_menu("item unregistered");
                        }
                        self.publish();
                    }
                }
                Err(err) => {
                    tracing::debug!(error = %err, "ignoring unregister for unparsable entry");
                }
            },

            Event::NameLost(name) => {
                let removed = self.registry.remove_for_lost_name(&name);
                if !removed.is_empty() {
                    for seq in &removed {
                        self.drop_tasks(*seq);
                    }
                    if self
                        .open_menu
                        .as_ref()
                        .is_some_and(|open| removed.contains(&open.seq))
                    {
                        self.invalidate_menu("owner disappeared");
                    }
                    self.publish();
                }
            }

            Event::Resolved {
                seq,
                generation,
                result,
            } => self.on_resolved(seq, generation, *result),

            Event::Changed { seq } => {
                self.cancel_resolve_retry(seq);
                let Some(generation) = self.registry.begin_refresh(seq) else {
                    return;
                };
                let Some(address) = self.registry.address_of(seq).cloned() else {
                    return;
                };
                self.start_resolve(connection, seq, generation, address);
            }

            Event::RetryResolve { seq, generation } => {
                self.retry_resolve(connection, seq, generation);
            }

            Event::Command(command) => self.dispatch(connection, command),

            Event::MenuLayout { token, result } => self.on_menu_layout(token, *result),

            Event::MenuChanged { token } => self.on_menu_changed(token),
        }
    }

    fn on_resolved(&mut self, seq: DiscoverySeq, generation: Generation, result: ResolveResult) {
        let (applied, unsettled) = match result {
            Ok(props) => {
                let unsettled =
                    icons::resolve(&props.icon, props.status, icons::IconKind::Primary, 1)
                        .is_empty();
                (
                    self.registry.apply_resolved(seq, generation, props),
                    unsettled,
                )
            }
            Err(reason) => (self.registry.apply_failure(seq, generation, reason), true),
        };
        if applied != Applied::Changed {
            return;
        }
        if let Some(tasks) = self.tasks.get_mut(&seq) {
            tasks.resolve = None;
        }
        if unsettled {
            self.schedule_resolve_retry(seq, generation);
        } else {
            self.cancel_resolve_retry(seq);
        }
        self.publish();
    }

    fn retry_resolve(
        &mut self,
        connection: &zbus::Connection,
        seq: DiscoverySeq,
        generation: Generation,
    ) {
        if self.registry.generation_of(seq) != Some(generation) {
            return;
        }
        if let Some(tasks) = self.tasks.get_mut(&seq) {
            tasks.retry = None;
        }
        let Some(next_generation) = self.registry.begin_refresh(seq) else {
            return;
        };
        let Some(address) = self.registry.address_of(seq).cloned() else {
            return;
        };
        self.start_resolve(connection, seq, next_generation, address);
    }

    fn on_menu_layout(&mut self, token: MenuToken, result: Result<(u32, RawLayout), String>) {
        let Some(open) = self.open_menu.as_ref().filter(|open| open.token == token) else {
            tracing::debug!("stale menu layout ignored");
            return;
        };

        match result {
            Ok((revision, layout)) => {
                let model = MenuModel::from_layout(
                    open.address.clone(),
                    open.item_generation,
                    revision,
                    &layout,
                );
                if model.is_empty() {
                    tracing::debug!(item = %open.address, "menu has nothing to show");
                    self.invalidate_menu("empty");
                    return;
                }
                tracing::debug!(
                    item = %open.address,
                    revision,
                    entries = model.entries.len(),
                    "menu ready"
                );
                let _ = self.menus.send(Some(Arc::new(model)));
            }
            Err(reason) => {
                tracing::warn!(item = %open.address, reason, "menu unavailable");
                self.invalidate_menu("layout failed");
            }
        }
    }

    fn on_menu_changed(&mut self, token: MenuToken) {
        let Some(open) = self.open_menu.as_ref().filter(|open| open.token == token) else {
            return;
        };
        self.fetch_layout(
            open.connection.clone(),
            open.address.clone(),
            open.menu_path.clone(),
            token,
            false,
        );
    }

    async fn introduce(
        &mut self,
        connection: &zbus::Connection,
        entry: &str,
    ) -> Option<ItemAddress> {
        let address = match host::resolve_address(connection, entry).await {
            Ok(address) => address,
            Err(err) => {
                tracing::debug!(entry, error = %err, "skipping item");
                return None;
            }
        };

        let Some((seq, _generation)) = self.registry.discover(address.clone()) else {
            return Some(address);
        };

        self.start_signal_watch(connection, seq, address.clone());
        Some(address)
    }

    fn start_resolve(
        &mut self,
        connection: &zbus::Connection,
        seq: DiscoverySeq,
        generation: Generation,
        address: ItemAddress,
    ) {
        let connection = connection.clone();
        let tx = self.internal_tx.clone();
        let handle = tokio::spawn(async move {
            let result = Box::pin(resolve(&connection, &address)).await;
            let _ = tx
                .send(Event::Resolved {
                    seq,
                    generation,
                    result: Box::new(result),
                })
                .await;
        });

        if let Some(tasks) = self.tasks.get_mut(&seq)
            && let Some(previous) = tasks.resolve.replace(handle)
        {
            previous.abort();
        }
    }

    fn schedule_resolve_retry(&mut self, seq: DiscoverySeq, generation: Generation) {
        let Some(tasks) = self.tasks.get_mut(&seq) else {
            return;
        };
        let Some(&delay) = RESOLVE_RETRY_DELAYS.get(tasks.retry_attempts) else {
            return;
        };

        if let Some(previous) = tasks.retry.take() {
            previous.abort();
        }
        tasks.retry_attempts += 1;
        let tx = self.internal_tx.clone();
        tasks.retry = Some(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(Event::RetryResolve { seq, generation }).await;
        }));
    }

    fn cancel_resolve_retry(&mut self, seq: DiscoverySeq) {
        if let Some(tasks) = self.tasks.get_mut(&seq) {
            if let Some(retry) = tasks.retry.take() {
                retry.abort();
            }
            tasks.retry_attempts = 0;
        }
    }

    fn start_signal_watch(
        &mut self,
        connection: &zbus::Connection,
        seq: DiscoverySeq,
        address: ItemAddress,
    ) {
        let connection = connection.clone();
        let tx = self.internal_tx.clone();
        let signals = tokio::spawn(async move {
            let subscribed = match item_proxy(&connection, &address).await {
                Ok(proxy) => {
                    #[allow(clippy::needless_continue)]
                    let merged = futures::stream_select!(
                        stream_or_empty(proxy.receive_new_icon().await).map(|_| ()),
                        stream_or_empty(proxy.receive_new_attention_icon().await).map(|_| ()),
                        stream_or_empty(proxy.receive_new_overlay_icon().await).map(|_| ()),
                        stream_or_empty(proxy.receive_new_title().await).map(|_| ()),
                        stream_or_empty(proxy.receive_new_tool_tip().await).map(|_| ()),
                        stream_or_empty(proxy.receive_new_status().await).map(|_| ()),
                    );
                    Some(merged)
                }
                Err(err) => {
                    tracing::debug!(item = %address, error = %err, "cannot watch item signals");
                    None
                }
            };

            if tx.send(Event::Changed { seq }).await.is_err() {
                return;
            }

            let Some(mut merged) = subscribed else {
                return;
            };

            while merged.next().await.is_some() {
                tracing::debug!(item = %address, "item announced a change");
                if tx.send(Event::Changed { seq }).await.is_err() {
                    return;
                }
            }
        });

        if let Some(previous) = self.tasks.insert(
            seq,
            ItemTasks {
                signals,
                resolve: None,
                retry: None,
                retry_attempts: 0,
            },
        ) {
            previous.abort();
        }
    }

    fn drop_tasks(&mut self, seq: DiscoverySeq) {
        if let Some(tasks) = self.tasks.remove(&seq) {
            tasks.abort();
        }
    }

    fn dispatch(&mut self, connection: &zbus::Connection, command: CoreCommand) {
        match command {
            CoreCommand::SetRemembered(order) => self.adopt_order(order),

            CoreCommand::Primary { address, token } => {
                let Some((_, _, _)) = self.item_for(&address) else {
                    return;
                };

                spawn_action(connection, address, move |proxy| async move {
                    if let Some(token) = token {
                        let _ = with_timeout(
                            ACTION_TIMEOUT,
                            "ProvideXdgActivationToken",
                            proxy.provide_xdg_activation_token(&token),
                        )
                        .await;
                    }
                    with_timeout(ACTION_TIMEOUT, "Activate", proxy.activate(0, 0)).await
                });
            }

            CoreCommand::Secondary { address } => {
                spawn_action(connection, address, |proxy| async move {
                    with_timeout(
                        ACTION_TIMEOUT,
                        "SecondaryActivate",
                        proxy.secondary_activate(0, 0),
                    )
                    .await
                });
            }

            CoreCommand::Context { address } => self.open_menu(connection, &address),

            CoreCommand::CloseMenu => self.close_menu(),

            CoreCommand::MenuClicked {
                address,
                id,
                token,
                close,
            } => {
                let Some((_, _, props)) = self.item_for(&address) else {
                    return;
                };
                let Some(menu_path) = props.menu_path else {
                    return;
                };
                if close
                    && self
                        .open_menu
                        .as_ref()
                        .is_some_and(|open| open.address == address)
                {
                    let open = self
                        .open_menu
                        .take()
                        .expect("the open menu was just matched");
                    open.watch.abort();
                    let _ = self.menus.send(None);
                }
                let connection = connection.clone();
                tokio::spawn(async move {
                    if let Some(token) = token
                        && let Ok(proxy) = item_proxy(&connection, &address).await
                    {
                        let _ = with_timeout(
                            ACTION_TIMEOUT,
                            "ProvideXdgActivationToken",
                            proxy.provide_xdg_activation_token(&token),
                        )
                        .await;
                    }
                    let Ok(proxy) = menu_proxy(&connection, &address, menu_path).await else {
                        return;
                    };
                    if let Err(err) = with_timeout(
                        ACTION_TIMEOUT,
                        "Event(clicked)",
                        proxy.event(id, "clicked", &zbus::zvariant::Value::I32(0), 0),
                    )
                    .await
                    {
                        tracing::warn!(item = %address, entry = id, error = %err, "menu click failed");
                    }
                    if close {
                        let _ = with_timeout(
                            ACTION_TIMEOUT,
                            "Event(closed)",
                            proxy.event(0, "closed", &zbus::zvariant::Value::I32(0), 0),
                        )
                        .await;
                    }
                });
            }
        }
    }

    fn item_for(&self, address: &ItemAddress) -> Option<(DiscoverySeq, Generation, ResolvedProps)> {
        let seq = self.registry.slot_for_address(address)?;
        Some((
            seq,
            self.registry.generation_of(seq)?,
            self.registry.props_of(seq)?.clone(),
        ))
    }

    fn open_menu(&mut self, connection: &zbus::Connection, address: &ItemAddress) {
        let Some((seq, generation, props)) = self.item_for(address) else {
            return;
        };

        let Some(menu_path) = props.menu_path else {
            return;
        };

        self.close_menu();

        self.next_menu_token += 1;
        let token = MenuToken(self.next_menu_token);

        let watch = self.spawn_menu_watch(connection, address, menu_path.clone(), token);
        self.open_menu = Some(OpenMenu {
            token,
            address: address.clone(),
            seq,
            item_generation: generation,
            menu_path: menu_path.clone(),
            connection: connection.clone(),
            watch,
        });

        tracing::debug!(item = %address, "menu opening");
        self.fetch_layout(connection.clone(), address.clone(), menu_path, token, true);
    }

    fn spawn_menu_watch(
        &self,
        connection: &zbus::Connection,
        address: &ItemAddress,
        menu_path: OwnedObjectPath,
        token: MenuToken,
    ) -> JoinHandle<()> {
        let tx = self.internal_tx.clone();
        let connection = connection.clone();
        let address = address.clone();

        tokio::spawn(async move {
            let Ok(proxy) = menu_proxy(&connection, &address, menu_path).await else {
                return;
            };

            let layout = stream_or_empty(proxy.receive_layout_updated().await).map(|_| ());
            let properties =
                stream_or_empty(proxy.receive_items_properties_updated().await).map(|_| ());

            #[allow(clippy::needless_continue)]
            let mut merged = futures::stream_select!(layout, properties);

            while merged.next().await.is_some() {
                if tx.send(Event::MenuChanged { token }).await.is_err() {
                    return;
                }
            }
        })
    }

    fn fetch_layout(
        &self,
        connection: zbus::Connection,
        address: ItemAddress,
        menu_path: OwnedObjectPath,
        token: MenuToken,
        announce: bool,
    ) {
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let proxy = match menu_proxy(&connection, &address, menu_path).await {
                Ok(proxy) => proxy,
                Err(err) => {
                    let _ = tx
                        .send(Event::MenuLayout {
                            token,
                            result: Box::new(Err(format!("cannot reach menu: {err}"))),
                        })
                        .await;
                    return;
                }
            };

            if announce {
                let _ = with_timeout(
                    ACTION_TIMEOUT,
                    "Event(opened)",
                    proxy.event(0, "opened", &zbus::zvariant::Value::I32(0), 0),
                )
                .await;
                let _ = with_timeout(ACTION_TIMEOUT, "AboutToShow", proxy.about_to_show(0)).await;
            }

            let result = with_timeout(LAYOUT_TIMEOUT, "GetLayout", proxy.get_layout(0, -1, &[]))
                .await
                .map_err(|err| err.to_string());
            let _ = tx
                .send(Event::MenuLayout {
                    token,
                    result: Box::new(result),
                })
                .await;
        });
    }

    fn close_menu(&mut self) {
        let Some(open) = self.open_menu.take() else {
            return;
        };
        open.watch.abort();

        tracing::debug!(item = %open.address, "menu closed");
        tokio::spawn(async move {
            let Ok(proxy) = open.proxy().await else {
                return;
            };
            let _ = with_timeout(
                ACTION_TIMEOUT,
                "Event(closed)",
                proxy.event(0, "closed", &zbus::zvariant::Value::I32(0), 0),
            )
            .await;
        });

        let _ = self.menus.send(None);
    }

    fn invalidate_menu(&mut self, reason: &str) {
        let Some(open) = self.open_menu.take() else {
            return;
        };
        open.watch.abort();
        tracing::info!(item = %open.address, reason, "menu invalidated");
        let _ = self.menus.send(None);
    }

    fn publish(&mut self) {
        let snapshot = self.registry.snapshot(self.watcher.clone());

        let merged = ordering::remembered_after(self.registry.remembered(), &snapshot.items);
        if merged != self.to_persist {
            self.to_persist = merged;
            self.persist_at
                .get_or_insert_with(|| Instant::now() + PERSIST_DEBOUNCE);
        }

        let _ = self.snapshots.send(Arc::new(snapshot));
    }

    fn adopt_order(&mut self, order: Vec<ItemKey>) {
        self.registry.set_remembered(order.clone());
        self.to_persist = order;
        let snapshot = self.registry.snapshot(self.watcher.clone());
        let _ = self.snapshots.send(Arc::new(snapshot));
    }

    fn flush_order(&mut self) {
        if self.persist_at.take().is_some() {
            self.store.store(&self.to_persist);
        }
    }
}

async fn connect_session() -> Option<zbus::Connection> {
    match zbus::Connection::session().await {
        Ok(connection) => Some(connection),
        Err(err) => {
            tracing::error!(error = %err, "cannot reach the session bus");
            None
        }
    }
}

async fn item_proxy(
    connection: &zbus::Connection,
    address: &ItemAddress,
) -> zbus::Result<StatusNotifierItemProxy<'static>> {
    StatusNotifierItemProxy::builder(connection)
        .destination(address.service.clone())?
        .path(address.path.clone())?
        // SNI implementations announce New* signals instead of PropertiesChanged.
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

async fn resolve(connection: &zbus::Connection, address: &ItemAddress) -> ResolveResult {
    let proxy = item_proxy(connection, address)
        .await
        .map_err(|err| format!("cannot build proxy: {err}"))?;

    let (
        id,
        title,
        status,
        icon_name,
        icon_pixmap,
        category,
        item_is_menu,
        menu,
        tooltip,
        theme_path,
        attention_icon_name,
        attention_icon_pixmap,
        overlay_icon_name,
        overlay_icon_pixmap,
    ) = futures::join!(
        with_timeout(PROPERTY_TIMEOUT, "Id", proxy.id()),
        with_timeout(PROPERTY_TIMEOUT, "Title", proxy.title()),
        with_timeout(PROPERTY_TIMEOUT, "Status", proxy.status()),
        with_timeout(PROPERTY_TIMEOUT, "IconName", proxy.icon_name()),
        with_timeout(PROPERTY_TIMEOUT, "IconPixmap", proxy.icon_pixmap()),
        with_timeout(PROPERTY_TIMEOUT, "Category", proxy.category()),
        with_timeout(PROPERTY_TIMEOUT, "ItemIsMenu", proxy.item_is_menu()),
        with_timeout(PROPERTY_TIMEOUT, "Menu", proxy.menu()),
        with_timeout(PROPERTY_TIMEOUT, "ToolTip", proxy.tool_tip()),
        with_timeout(PROPERTY_TIMEOUT, "IconThemePath", proxy.icon_theme_path()),
        with_timeout(
            PROPERTY_TIMEOUT,
            "AttentionIconName",
            proxy.attention_icon_name()
        ),
        with_timeout(
            PROPERTY_TIMEOUT,
            "AttentionIconPixmap",
            proxy.attention_icon_pixmap()
        ),
        with_timeout(
            PROPERTY_TIMEOUT,
            "OverlayIconName",
            proxy.overlay_icon_name()
        ),
        with_timeout(
            PROPERTY_TIMEOUT,
            "OverlayIconPixmap",
            proxy.overlay_icon_pixmap()
        ),
    );

    let identifying: [Option<&CallError>; 5] = [
        id.as_ref().err(),
        title.as_ref().err(),
        status.as_ref().err(),
        icon_name.as_ref().err(),
        icon_pixmap.as_ref().err(),
    ];
    if identifying.iter().all(Option::is_some) {
        let reason = identifying
            .iter()
            .flatten()
            .map(ToString::to_string)
            .next()
            .unwrap_or_else(|| "item did not answer".to_owned());
        return Err(reason);
    }

    let menu_path = menu.ok().filter(|path| path.as_str() != "/");
    let theme_path = theme_path.ok().filter(|path| !path.is_empty());

    Ok(ResolvedProps {
        id: id.unwrap_or_default(),
        title: title.unwrap_or_default(),
        category: category.as_deref().map(Category::from).unwrap_or_default(),
        status: status.as_deref().map(ItemStatus::from).unwrap_or_default(),
        item_is_menu: item_is_menu.unwrap_or(false),
        menu_path,
        tooltip: tooltip.ok(),
        icon: Arc::new(IconSource {
            icon_name: icon_name.unwrap_or_default(),
            icon_pixmap: icon_pixmap.unwrap_or_default(),
            attention_icon_name: attention_icon_name.unwrap_or_default(),
            attention_icon_pixmap: attention_icon_pixmap.unwrap_or_default(),
            overlay_icon_name: overlay_icon_name.unwrap_or_default(),
            overlay_icon_pixmap: overlay_icon_pixmap.unwrap_or_default(),
            theme_path,
        }),
    })
}

fn spawn_action<F, Fut>(connection: &zbus::Connection, address: ItemAddress, action: F)
where
    F: FnOnce(StatusNotifierItemProxy<'static>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), CallError>> + Send,
{
    let connection = connection.clone();
    tokio::spawn(async move {
        let proxy = match item_proxy(&connection, &address).await {
            Ok(proxy) => proxy,
            Err(err) => {
                tracing::warn!(item = %address, error = %err, "cannot reach item");
                return;
            }
        };
        if let Err(err) = action(proxy).await {
            tracing::warn!(item = %address, error = %err, "item action failed");
        }
    });
}

fn stream_or_empty<T, S>(
    result: zbus::Result<S>,
) -> futures::future::Either<S, futures::stream::Empty<T>>
where
    S: futures::Stream<Item = T>,
{
    match result {
        Ok(stream) => stream.left_stream(),
        Err(err) => {
            tracing::debug!(error = %err, "signal subscription failed");
            futures::stream::empty().right_stream()
        }
    }
}

fn split(
    link: Option<host::WatcherLink>,
) -> (
    Option<proxies::StatusNotifierItemRegisteredStream>,
    Option<proxies::StatusNotifierItemUnregisteredStream>,
) {
    match link {
        Some(link) => (Some(link.registered), Some(link.unregistered)),
        None => (None, None),
    }
}

async fn next_or_pending<S: futures::Stream + Unpin>(stream: Option<&mut S>) -> Option<S::Item> {
    match stream {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
    }
}

async fn menu_proxy(
    connection: &zbus::Connection,
    address: &ItemAddress,
    menu_path: OwnedObjectPath,
) -> zbus::Result<DBusMenuProxy<'static>> {
    DBusMenuProxy::builder(connection)
        .destination(address.service.clone())?
        .path(menu_path)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}
