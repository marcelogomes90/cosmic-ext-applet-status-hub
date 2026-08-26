use std::sync::Arc;

use tokio::sync::Mutex;
use zbus::names::OwnedUniqueName;
use zbus::object_server::SignalEmitter;

const PATH: &str = "/StatusNotifierWatcher";
const COSMIC_PATH: &str = "/CosmicStatusNotifierWatcher";

#[derive(Debug, Default)]
struct Items(Vec<(OwnedUniqueName, String)>);

#[derive(Clone, Debug, Default)]
pub struct FakeWatcher {
    items: Arc<Mutex<Items>>,
    hosts: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone, Debug, Default)]
pub struct FakeCosmicWatcher {
    clients: Arc<Mutex<Vec<OwnedUniqueName>>>,
}

#[zbus::interface(name = "com.system76.CosmicStatusNotifierWatcher")]
impl FakeCosmicWatcher {
    async fn register_applet(&self, #[zbus(header)] header: zbus::message::Header<'_>) {
        let Some(sender) = header.sender() else {
            return;
        };
        let sender = sender.to_owned().into();
        let mut clients = self.clients.lock().await;
        if !clients.contains(&sender) {
            clients.push(sender);
        }
    }
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl FakeWatcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let Some(sender) = header.sender() else {
            return;
        };
        let entry = if service.starts_with('/') {
            format!("{sender}{service}")
        } else {
            service.to_owned()
        };

        let mut items = self.items.lock().await;
        if items.0.iter().any(|(_, known)| known == &entry) {
            return;
        }
        items.0.push((sender.to_owned().into(), entry.clone()));
        drop(items);

        let _ = Self::status_notifier_item_registered(&emitter, &entry).await;
    }

    async fn register_status_notifier_host(&self, service: &str) {
        self.hosts.lock().await.push(service.to_owned());
    }

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items
            .lock()
            .await
            .0
            .iter()
            .map(|(_, entry)| entry.clone())
            .collect()
    }

    #[zbus(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        !self.hosts.lock().await.is_empty()
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;
}

impl FakeCosmicWatcher {
    pub async fn serve(connection: &zbus::Connection) -> zbus::Result<Self> {
        let watcher = Self::default();
        connection
            .object_server()
            .at(COSMIC_PATH, watcher.clone())
            .await?;
        connection
            .request_name("com.system76.CosmicStatusNotifierWatcher")
            .await?;
        Ok(watcher)
    }

    pub async fn clients(&self) -> Vec<OwnedUniqueName> {
        self.clients.lock().await.clone()
    }
}

impl FakeWatcher {
    pub async fn serve(connection: &zbus::Connection) -> zbus::Result<Self> {
        let watcher = Self::default();
        connection.object_server().at(PATH, watcher.clone()).await?;
        connection
            .request_name("org.kde.StatusNotifierWatcher")
            .await?;
        Ok(watcher)
    }

    pub async fn hosts(&self) -> Vec<String> {
        self.hosts.lock().await.clone()
    }

    pub async fn unregister(&self, connection: &zbus::Connection, entry: &str) -> zbus::Result<()> {
        self.items
            .lock()
            .await
            .0
            .retain(|(_, known)| known != entry);
        let emitter = SignalEmitter::new(connection, PATH)?;
        Self::status_notifier_item_unregistered(&emitter, entry).await
    }
}
