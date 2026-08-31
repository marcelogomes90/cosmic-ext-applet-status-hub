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
use crate::core::proxies::{
    DBusMenuProxy, IntrospectableProxy, RawLayout, StatusNotifierItemProxy,
};
use crate::core::registry::{Applied, Registry, ResolvedProps};
use zbus::zvariant::OwnedObjectPath;

const PERSIST_DEBOUNCE: Duration = Duration::from_secs(2);
const RESOLVE_RETRY_DELAYS: [Duration; 8] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(8),
    Duration::from_secs(20),
    Duration::from_secs(45),
    Duration::from_secs(90),
    Duration::from_mins(3),
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

struct Resolved {
    props: ResolvedProps,
    partial: bool,
}

type ResolveResult = Result<Resolved, String>;

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
    fetch: MenuFetchState,
}

#[derive(Debug, Default)]
struct MenuFetchState {
    in_flight: bool,
    pending: bool,
    last_revision: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MenuFetchCompletion {
    accept: bool,
    fetch_again: bool,
}

impl MenuFetchState {
    fn request(&mut self) -> bool {
        if self.in_flight {
            self.pending = true;
            false
        } else {
            self.in_flight = true;
            true
        }
    }

    fn complete(&mut self, revision: u32) -> MenuFetchCompletion {
        self.in_flight = false;
        let accept = self.last_revision.is_none_or(|current| revision >= current);
        if accept {
            self.last_revision = Some(revision);
        }
        MenuFetchCompletion {
            accept,
            fetch_again: std::mem::take(&mut self.pending),
        }
    }
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
            Ok(Resolved { props, partial }) => {
                let unsettled = partial
                    || icons::resolve(&props.icon, props.status, icons::IconKind::Primary, 1)
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
        let Some(open) = self.open_menu.as_mut().filter(|open| open.token == token) else {
            tracing::debug!("stale menu layout ignored");
            return;
        };
        match result {
            Ok((revision, layout)) => {
                let completion = open.fetch.complete(revision);
                if !completion.accept {
                    tracing::debug!(revision, "out-of-order menu layout ignored");
                    if completion.fetch_again {
                        self.start_menu_fetch(token, false);
                    }
                    return;
                }
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
                if completion.fetch_again {
                    self.start_menu_fetch(token, false);
                }
            }
            Err(reason) => {
                open.fetch.in_flight = false;
                tracing::warn!(item = %open.address, reason, "menu unavailable");
                self.invalidate_menu("layout failed");
            }
        }
    }

    fn on_menu_changed(&mut self, token: MenuToken) {
        self.start_menu_fetch(token, false);
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
                        stream_or_empty(proxy.receive_new_icon_theme_path().await).map(|_| ()),
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
            CoreCommand::SetRemembered(order) => self.adopt_order(&order),

            CoreCommand::Primary { address, token } => {
                self.dispatch_primary(connection, address, token);
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

            CoreCommand::Context { address } => self.dispatch_context(connection, address),

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
                        offer_token(&proxy, &token).await;
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

    fn dispatch_context(&mut self, connection: &zbus::Connection, address: ItemAddress) {
        let Some((seq, _, props)) = self.item_for(&address) else {
            return;
        };
        if props.menu_path.is_some() {
            self.open_menu(connection, &address);
        } else {
            let tx = self.internal_tx.clone();
            spawn_action(connection, address, move |proxy| async move {
                let result =
                    with_timeout(ACTION_TIMEOUT, "ContextMenu", proxy.context_menu(0, 0)).await;
                if result.as_ref().is_err_and(|err| !err.is_transient()) {
                    let _ = tx.send(Event::Changed { seq }).await;
                }
                result
            });
        }
    }

    fn dispatch_primary(
        &mut self,
        connection: &zbus::Connection,
        address: ItemAddress,
        token: Option<String>,
    ) {
        if self.item_for(&address).is_none() {
            return;
        }

        if token.is_none() {
            tracing::debug!(item = %address, "activating without an activation token");
        }

        spawn_action(connection, address, move |proxy| async move {
            if let Some(token) = token {
                offer_token(&proxy, &token).await;
            }
            with_timeout(ACTION_TIMEOUT, "Activate", proxy.activate(0, 0)).await
        });
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
            fetch: MenuFetchState::default(),
        });

        tracing::debug!(item = %address, "menu opening");
        self.start_menu_fetch(token, true);
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

    fn start_menu_fetch(&mut self, token: MenuToken, announce: bool) {
        let Some(open) = self.open_menu.as_mut().filter(|open| open.token == token) else {
            return;
        };
        if !open.fetch.request() {
            return;
        }
        let connection = open.connection.clone();
        let address = open.address.clone();
        let menu_path = open.menu_path.clone();
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
                let announce_proxy = proxy.clone();
                let announce_tx = tx.clone();
                tokio::spawn(async move {
                    let opened = with_timeout(
                        ACTION_TIMEOUT,
                        "Event(opened)",
                        announce_proxy.event(0, "opened", &zbus::zvariant::Value::I32(0), 0),
                    );
                    let about_to_show = with_timeout(
                        ACTION_TIMEOUT,
                        "AboutToShow",
                        announce_proxy.about_to_show(0),
                    );
                    let _ = futures::join!(opened, about_to_show);
                    let _ = announce_tx.send(Event::MenuChanged { token }).await;
                });
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

    fn adopt_order(&mut self, order: &[ItemKey]) {
        let merged = ordering::merge_remembered(order, self.registry.remembered());
        if merged == self.to_persist && merged == self.registry.remembered() {
            return;
        }

        self.registry.set_remembered(merged.clone());
        self.to_persist = merged;
        self.persist_at
            .get_or_insert_with(|| Instant::now() + PERSIST_DEBOUNCE);

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
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

async fn properties_proxy(
    connection: &zbus::Connection,
    address: &ItemAddress,
) -> zbus::Result<zbus::fdo::PropertiesProxy<'static>> {
    zbus::fdo::PropertiesProxy::builder(connection)
        .destination(address.service.clone())?
        .path(address.path.clone())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

const SNI_INTERFACE: &str = "org.kde.StatusNotifierItem";

const IDENTIFYING: [&str; 5] = ["Id", "Title", "Status", "IconName", "IconPixmap"];

const PROPERTIES: [&str; 13] = [
    "Id",
    "Title",
    "Status",
    "IconName",
    "IconPixmap",
    "Category",
    "Menu",
    "ToolTip",
    "IconThemePath",
    "AttentionIconName",
    "AttentionIconPixmap",
    "OverlayIconName",
    "OverlayIconPixmap",
];

type Properties = HashMap<String, zbus::zvariant::OwnedValue>;

fn sni_interface() -> zbus::names::InterfaceName<'static> {
    zbus::names::InterfaceName::from_static_str_unchecked(SNI_INTERFACE)
}

async fn get_all_properties(
    proxy: &zbus::fdo::PropertiesProxy<'_>,
    failures: &mut Vec<CallError>,
) -> Properties {
    match with_timeout(PROPERTY_TIMEOUT, "GetAll", proxy.get_all(sni_interface())).await {
        Ok(bulk) => bulk,
        Err(err) => {
            tracing::debug!(error = %err, "GetAll unavailable, falling back to single gets");
            failures.push(err);
            Properties::new()
        }
    }
}

async fn fill_missing_properties(
    proxy: &zbus::fdo::PropertiesProxy<'_>,
    bulk: &mut Properties,
    declared: &std::collections::HashSet<String>,
    failures: &mut Vec<CallError>,
) {
    for name in PROPERTIES {
        if bulk.contains_key(name) || (!declared.is_empty() && !declared.contains(name)) {
            continue;
        }
        match with_timeout(PROPERTY_TIMEOUT, name, proxy.get(sni_interface(), name)).await {
            Ok(value) => {
                bulk.insert(name.to_owned(), value);
            }
            Err(err) => failures.push(err),
        }
    }
}

fn take<T: TryFrom<zbus::zvariant::OwnedValue>>(bulk: &mut Properties, name: &str) -> Option<T> {
    T::try_from(bulk.remove(name)?).ok()
}

async fn resolve(connection: &zbus::Connection, address: &ItemAddress) -> ResolveResult {
    let proxy = properties_proxy(connection, address)
        .await
        .map_err(|err| format!("cannot build proxy: {err}"))?;

    let mut failures = Vec::new();
    let mut bulk = get_all_properties(&proxy, &mut failures).await;
    let introspection = introspect(connection, address).await;
    let declared = declared_properties(introspection.as_deref());
    fill_missing_properties(&proxy, &mut bulk, &declared, &mut failures).await;

    if IDENTIFYING.iter().all(|name| !bulk.contains_key(*name)) {
        return Err(failures
            .first()
            .map_or_else(|| "item did not answer".to_owned(), ToString::to_string));
    }

    let missing = declared
        .iter()
        .any(|name| PROPERTIES.contains(&name.as_str()) && !bulk.contains_key(name));
    let partial = missing || failures.iter().any(CallError::is_transient);

    let menu_path: Option<OwnedObjectPath> =
        take(&mut bulk, "Menu").filter(|path: &OwnedObjectPath| path.as_str() != "/");
    let theme_path: Option<String> =
        take(&mut bulk, "IconThemePath").filter(|path: &String| !path.is_empty());
    let status: Option<String> = take(&mut bulk, "Status");
    let category: Option<String> = take(&mut bulk, "Category");

    Ok(Resolved {
        partial,
        props: ResolvedProps {
            id: take(&mut bulk, "Id").unwrap_or_default(),
            title: take(&mut bulk, "Title").unwrap_or_default(),
            category: category.as_deref().map(Category::from).unwrap_or_default(),
            status: status.as_deref().map(ItemStatus::from).unwrap_or_default(),
            menu_path,
            tooltip: take(&mut bulk, "ToolTip"),
            takes_activation_token: announces_activation_token(introspection.as_deref()),
            icon: Arc::new(IconSource {
                icon_name: take(&mut bulk, "IconName").unwrap_or_default(),
                icon_pixmap: take(&mut bulk, "IconPixmap").unwrap_or_default(),
                attention_icon_name: take(&mut bulk, "AttentionIconName").unwrap_or_default(),
                attention_icon_pixmap: take(&mut bulk, "AttentionIconPixmap").unwrap_or_default(),
                overlay_icon_name: take(&mut bulk, "OverlayIconName").unwrap_or_default(),
                overlay_icon_pixmap: take(&mut bulk, "OverlayIconPixmap").unwrap_or_default(),
                theme_path,
            }),
        },
    })
}

async fn introspect(connection: &zbus::Connection, address: &ItemAddress) -> Option<String> {
    let proxy = IntrospectableProxy::builder(connection)
        .destination(address.service.clone())
        .ok()?
        .path(address.path.clone())
        .ok()?
        .build()
        .await
        .ok()?;

    with_timeout(PROPERTY_TIMEOUT, "Introspect", proxy.introspect())
        .await
        .ok()
}

fn announces_activation_token(introspection: Option<&str>) -> bool {
    introspection.is_some_and(|xml| xml.contains("ProvideXdgActivationToken"))
}

fn declared_properties(introspection: Option<&str>) -> std::collections::HashSet<String> {
    let Some(xml) = introspection else {
        return std::collections::HashSet::new();
    };
    let Some(interface) = xml.split(SNI_INTERFACE).nth(1) else {
        return std::collections::HashSet::new();
    };
    let interface = interface.split("<interface").next().unwrap_or(interface);

    interface
        .match_indices("<property")
        .filter_map(|(at, _)| {
            let rest = &interface[at..];
            let end = rest.find('>')?;
            let name = rest[..end].split("name=").nth(1)?.trim_start();
            let quote = name.chars().next().filter(|ch| *ch == '"' || *ch == '\'')?;
            let name = &name[1..];
            Some(name[..name.find(quote)?].to_owned())
        })
        .collect()
}

async fn offer_token(proxy: &StatusNotifierItemProxy<'_>, token: &str) {
    if let Err(err) = with_timeout(
        ACTION_TIMEOUT,
        "ProvideXdgActivationToken",
        proxy.provide_xdg_activation_token(token),
    )
    .await
    {
        tracing::debug!(
            item = %proxy.inner().destination(),
            error = %err,
            "the item takes no activation token, so it cannot raise its own window"
        );
    }
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

#[cfg(test)]
mod token_capability_tests {
    use super::announces_activation_token;

    const QT_ITEM: &str = r#"<node>
  <interface name="org.kde.StatusNotifierItem">
    <method name="Activate"><arg name="x" type="i" direction="in"/></method>
    <method name="ProvideXdgActivationToken"><arg name="token" type="s" direction="in"/></method>
  </interface>
</node>"#;

    const ELECTRON_ITEM: &str = r#"<node>
  <interface name="org.kde.StatusNotifierItem">
    <method name="Activate"><arg name="x" type="i" direction="in"/></method>
    <method name="SecondaryActivate"><arg name="x" type="i" direction="in"/></method>
  </interface>
</node>"#;

    #[test]
    fn an_item_that_lists_the_method_is_left_to_raise_itself() {
        assert!(announces_activation_token(Some(QT_ITEM)));
    }

    #[test]
    fn an_item_without_the_method_needs_the_applet() {
        assert!(!announces_activation_token(Some(ELECTRON_ITEM)));
    }

    #[test]
    fn a_sandbox_that_hides_the_interface_is_treated_as_needing_the_applet() {
        assert!(!announces_activation_token(Some("<node>\n</node>")));
        assert!(!announces_activation_token(Some("")));
        assert!(!announces_activation_token(None));
    }
}

#[cfg(test)]
mod declared_property_tests {
    use super::declared_properties;

    const AYATANA_ITEM: &str = r#"<node>
  <interface name="org.freedesktop.DBus.Properties">
    <property name="Decoy" type="s" access="read"/>
  </interface>
  <interface name="org.kde.StatusNotifierItem">
    <method name="Scroll"><arg name="delta" type="i" direction="in"/></method>
    <property name="IconName" type="s" access="read"/>
    <property name='IconThemePath' type="s" access="read"/>
    <property name="Menu" type="o" access="read"/>
  </interface>
  <interface name="org.freedesktop.DBus.Peer">
    <property name="Other" type="s" access="read"/>
  </interface>
</node>"#;

    #[test]
    fn only_the_tray_interfaces_properties_are_collected() {
        let declared = declared_properties(Some(AYATANA_ITEM));
        assert_eq!(declared.len(), 3, "got {declared:?}");
        for name in ["IconName", "IconThemePath", "Menu"] {
            assert!(
                declared.contains(name),
                "{name} is missing from {declared:?}"
            );
        }
    }

    #[test]
    fn an_item_that_lists_no_properties_says_nothing_about_what_it_has() {
        assert!(declared_properties(None).is_empty());
        assert!(declared_properties(Some("<node>\n</node>")).is_empty());
        assert!(
            declared_properties(Some(
                r#"<node><interface name="org.kde.StatusNotifierItem"></interface></node>"#
            ))
            .is_empty()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_layout_updates_are_coalesced_while_a_fetch_is_running() {
        let mut state = MenuFetchState::default();

        assert!(state.request());
        assert!(!state.request());
        assert!(!state.request());
        assert_eq!(
            state.complete(4),
            MenuFetchCompletion {
                accept: true,
                fetch_again: true,
            }
        );
        assert!(state.request());
        assert_eq!(
            state.complete(5),
            MenuFetchCompletion {
                accept: true,
                fetch_again: false,
            }
        );
    }

    #[test]
    fn an_older_menu_revision_never_replaces_the_current_one() {
        let mut state = MenuFetchState::default();

        assert!(state.request());
        assert!(state.complete(8).accept);
        assert!(state.request());
        assert!(!state.complete(7).accept);
        assert_eq!(state.last_revision, Some(8));
    }
}
