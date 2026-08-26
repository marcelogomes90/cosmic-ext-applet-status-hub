use std::time::Duration;

use futures::StreamExt;
use zbus::fdo::DBusProxy;
use zbus::names::{BusName, WellKnownName};

use crate::core::call::{self, CallError, PROPERTY_TIMEOUT};
use crate::core::model::{ItemAddress, parse_service_entry};
use crate::core::proxies::{
    CosmicStatusNotifierWatcherProxy, StatusNotifierItemRegisteredStream,
    StatusNotifierItemUnregisteredStream, StatusNotifierWatcherProxy,
};

pub const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const COSMIC_WATCHER_NAME: &str = "com.system76.CosmicStatusNotifierWatcher";

pub struct WatcherLink {
    pub proxy: StatusNotifierWatcherProxy<'static>,
    pub registered: StatusNotifierItemRegisteredStream,
    pub unregistered: StatusNotifierItemUnregisteredStream,
    pub initial: Vec<String>,
}

pub async fn connect(connection: &zbus::Connection) -> Result<WatcherLink, CallError> {
    let proxy = call::with_timeout(
        PROPERTY_TIMEOUT,
        "watcher connect",
        StatusNotifierWatcherProxy::builder(connection)
            .destination(WATCHER_NAME)
            .expect("watcher name is a valid bus name")
            .path(WATCHER_PATH)
            .expect("watcher path is a valid object path")
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build(),
    )
    .await?;

    let registered = call::with_timeout(
        PROPERTY_TIMEOUT,
        "subscribe StatusNotifierItemRegistered",
        proxy.receive_status_notifier_item_registered(),
    )
    .await?;
    let unregistered = call::with_timeout(
        PROPERTY_TIMEOUT,
        "subscribe StatusNotifierItemUnregistered",
        proxy.receive_status_notifier_item_unregistered(),
    )
    .await?;

    let host_name = connection
        .unique_name()
        .expect("a bus connection has a unique name")
        .as_str();
    if let Err(err) = call::with_timeout(
        PROPERTY_TIMEOUT,
        "RegisterStatusNotifierHost",
        proxy.register_status_notifier_host(host_name),
    )
    .await
    {
        tracing::debug!(error = %err, "watcher did not accept host registration");
    }

    let initial = call::with_timeout(
        PROPERTY_TIMEOUT,
        "RegisteredStatusNotifierItems",
        proxy.registered_status_notifier_items(),
    )
    .await?;

    tracing::info!(items = initial.len(), "connected to StatusNotifierWatcher");

    Ok(WatcherLink {
        proxy,
        registered,
        unregistered,
        initial,
    })
}

const WATCHER_PROVIDERS: [&str; 2] = [WATCHER_NAME, COSMIC_WATCHER_NAME];

const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(2);
const ACQUIRE_POLL: Duration = Duration::from_millis(100);

pub async fn try_activate(connection: &zbus::Connection) -> Result<(), CallError> {
    let dbus =
        call::with_timeout(PROPERTY_TIMEOUT, "DBusProxy", DBusProxy::new(connection)).await?;
    let name = WellKnownName::try_from(WATCHER_NAME).expect("constant is a valid well-known name");

    if call::with_timeout(
        PROPERTY_TIMEOUT,
        "NameHasOwner",
        dbus.name_has_owner(name.clone().into()),
    )
    .await?
    {
        return register_cosmic_client(connection, &dbus).await;
    }

    let activatable = call::with_timeout(
        PROPERTY_TIMEOUT,
        "ListActivatableNames",
        dbus.list_activatable_names(),
    )
    .await?;

    let Some(provider) = WATCHER_PROVIDERS.iter().find(|candidate| {
        activatable
            .iter()
            .any(|available| available.as_str() == **candidate)
    }) else {
        tracing::warn!("no StatusNotifierWatcher is running and none can be started on this bus");
        return Ok(());
    };

    tracing::info!(provider, "watcher unavailable, requesting activation");
    let provider_name =
        WellKnownName::try_from(*provider).expect("candidate came from the bus's own list");
    call::with_timeout(
        PROPERTY_TIMEOUT,
        "StartServiceByName",
        dbus.start_service_by_name(provider_name, 0),
    )
    .await?;

    let deadline = tokio::time::Instant::now() + ACQUIRE_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if call::with_timeout(
            PROPERTY_TIMEOUT,
            "NameHasOwner",
            dbus.name_has_owner(name.clone().into()),
        )
        .await?
        {
            return register_cosmic_client(connection, &dbus).await;
        }
        tokio::time::sleep(ACQUIRE_POLL).await;
    }

    tracing::warn!(
        provider,
        "watcher was started but never claimed {WATCHER_NAME}"
    );
    Ok(())
}

async fn register_cosmic_client(
    connection: &zbus::Connection,
    dbus: &DBusProxy<'_>,
) -> Result<(), CallError> {
    let standard =
        WellKnownName::try_from(WATCHER_NAME).expect("watcher name is a valid well-known name");
    let cosmic = WellKnownName::try_from(COSMIC_WATCHER_NAME)
        .expect("COSMIC watcher name is a valid well-known name");

    let standard_owner = call::with_timeout(
        PROPERTY_TIMEOUT,
        "GetNameOwner(StatusNotifierWatcher)",
        dbus.get_name_owner(standard.into()),
    )
    .await?;
    let cosmic_has_owner = call::with_timeout(
        PROPERTY_TIMEOUT,
        "NameHasOwner(CosmicStatusNotifierWatcher)",
        dbus.name_has_owner(cosmic.clone().into()),
    )
    .await?;
    if !cosmic_has_owner {
        return Ok(());
    }
    let cosmic_owner = call::with_timeout(
        PROPERTY_TIMEOUT,
        "GetNameOwner(CosmicStatusNotifierWatcher)",
        dbus.get_name_owner(cosmic.into()),
    )
    .await?;

    if standard_owner != cosmic_owner {
        return Ok(());
    }

    let proxy = call::with_timeout(
        PROPERTY_TIMEOUT,
        "COSMIC watcher connect",
        CosmicStatusNotifierWatcherProxy::new(connection),
    )
    .await?;
    call::with_timeout(PROPERTY_TIMEOUT, "RegisterApplet", proxy.register_applet()).await
}

pub async fn resolve_address(
    connection: &zbus::Connection,
    entry: &str,
) -> Result<ItemAddress, ResolveAddressError> {
    let (service, path) =
        parse_service_entry(entry).map_err(|err| ResolveAddressError::Parse(err.to_string()))?;

    let dbus = DBusProxy::new(connection)
        .await
        .map_err(|err| ResolveAddressError::Owner(err.to_string()))?;
    let owner = call::with_timeout(
        PROPERTY_TIMEOUT,
        "GetNameOwner",
        dbus.get_name_owner(service.inner().clone()),
    )
    .await
    .map_err(|err| ResolveAddressError::Owner(err.to_string()))?;

    Ok(ItemAddress {
        service,
        path,
        owner,
    })
}

#[derive(Debug)]
pub enum ResolveAddressError {
    Parse(String),
    Owner(String),
}

impl std::fmt::Display for ResolveAddressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(msg) => write!(f, "malformed watcher entry: {msg}"),
            Self::Owner(msg) => write!(f, "owner unavailable: {msg}"),
        }
    }
}

impl std::error::Error for ResolveAddressError {}

pub async fn lost_name_stream(
    connection: &zbus::Connection,
) -> zbus::Result<impl futures::Stream<Item = BusName<'static>> + Send + use<>> {
    let dbus = DBusProxy::new(connection).await?;
    let stream = dbus.receive_name_owner_changed().await?;

    Ok(stream.filter_map(|signal| async move {
        let args = signal.args().ok()?;
        args.new_owner.is_none().then(|| args.name.to_owned())
    }))
}
