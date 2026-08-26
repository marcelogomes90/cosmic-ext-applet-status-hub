use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use crate::core::model::Pixmap;
use crate::testkit::ITEM_PATH;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemBehaviour {
    #[default]
    Normal,
    Hangs,
    Broken,
    NoPrimaryAction,
    ItemIsMenu,
    NoMenu,
}

#[derive(Debug)]
struct State {
    id: String,
    title: String,
    status: String,
    icon_name: String,
    behaviour: ItemBehaviour,
    activate_calls: u32,
    secondary_calls: u32,
    context_calls: u32,
    activation_tokens: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FakeItem(Arc<Mutex<State>>);

#[derive(Clone, Debug)]
pub struct ItemHandle {
    item: FakeItem,
    connection: zbus::Connection,
    pub registration: String,
}

impl FakeItem {
    async fn stall(&self) -> Option<zbus::fdo::Result<()>> {
        let behaviour = self.0.lock().await.behaviour;
        match behaviour {
            ItemBehaviour::Normal
            | ItemBehaviour::NoPrimaryAction
            | ItemBehaviour::ItemIsMenu
            | ItemBehaviour::NoMenu => None,
            ItemBehaviour::Broken => {
                Some(Err(zbus::fdo::Error::Failed("deliberately broken".into())))
            }
            ItemBehaviour::Hangs => {
                tokio::time::sleep(Duration::from_mins(10)).await;
                Some(Err(zbus::fdo::Error::Failed("unreachable".into())))
            }
        }
    }
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl FakeItem {
    #[zbus(property)]
    async fn id(&self) -> zbus::fdo::Result<String> {
        if let Some(err) = self.stall().await {
            err?;
        }
        Ok(self.0.lock().await.id.clone())
    }

    #[zbus(property)]
    async fn title(&self) -> zbus::fdo::Result<String> {
        if let Some(err) = self.stall().await {
            err?;
        }
        Ok(self.0.lock().await.title.clone())
    }

    #[zbus(property)]
    async fn status(&self) -> zbus::fdo::Result<String> {
        if let Some(err) = self.stall().await {
            err?;
        }
        Ok(self.0.lock().await.status.clone())
    }

    #[zbus(property)]
    async fn icon_name(&self) -> zbus::fdo::Result<String> {
        if let Some(err) = self.stall().await {
            err?;
        }
        Ok(self.0.lock().await.icon_name.clone())
    }

    #[zbus(property)]
    async fn icon_pixmap(&self) -> zbus::fdo::Result<Vec<Pixmap>> {
        if let Some(err) = self.stall().await {
            err?;
        }
        Ok(Vec::new())
    }

    #[zbus(property)]
    fn category(&self) -> String {
        "ApplicationStatus".to_owned()
    }

    #[zbus(property)]
    async fn item_is_menu(&self) -> bool {
        self.0.lock().await.behaviour == ItemBehaviour::ItemIsMenu
    }

    #[zbus(property)]
    async fn menu(&self) -> OwnedObjectPath {
        let path = if self.0.lock().await.behaviour == ItemBehaviour::NoMenu {
            "/"
        } else {
            "/MenuBar"
        };
        OwnedObjectPath::from(ObjectPath::try_from(path).expect("constant path"))
    }

    async fn activate(&self, _x: i32, _y: i32) -> zbus::fdo::Result<()> {
        let mut state = self.0.lock().await;
        state.activate_calls += 1;
        if state.behaviour == ItemBehaviour::NoPrimaryAction {
            return Err(zbus::fdo::Error::UnknownMethod(
                "No such method Activate".into(),
            ));
        }
        Ok(())
    }

    async fn secondary_activate(&self, _x: i32, _y: i32) {
        self.0.lock().await.secondary_calls += 1;
    }

    async fn context_menu(&self, _x: i32, _y: i32) {
        self.0.lock().await.context_calls += 1;
    }

    async fn provide_xdg_activation_token(&self, token: &str) {
        self.0.lock().await.activation_tokens.push(token.to_owned());
    }

    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
}

impl ItemHandle {
    pub async fn publish(
        connection: &zbus::Connection,
        id: &str,
        behaviour: ItemBehaviour,
        well_known: Option<&str>,
    ) -> zbus::Result<Self> {
        let item = FakeItem(Arc::new(Mutex::new(State {
            id: id.to_owned(),
            title: id.to_owned(),
            status: "Active".to_owned(),
            icon_name: "application-default".to_owned(),
            behaviour,
            activate_calls: 0,
            secondary_calls: 0,
            context_calls: 0,
            activation_tokens: Vec::new(),
        })));

        connection
            .object_server()
            .at(ITEM_PATH, item.clone())
            .await?;

        let registration = match well_known {
            Some(name) => {
                connection.request_name(name).await?;
                name.to_owned()
            }
            None => ITEM_PATH.to_owned(),
        };

        let watcher = crate::core::proxies::StatusNotifierWatcherProxy::new(connection).await?;
        watcher.register_status_notifier_item(&registration).await?;

        let registration = match well_known {
            Some(name) => name.to_owned(),
            None => format!(
                "{}{ITEM_PATH}",
                connection.unique_name().expect("connected to a bus")
            ),
        };

        Ok(Self {
            item,
            connection: connection.clone(),
            registration,
        })
    }

    pub async fn set_title(&self, title: &str) -> zbus::Result<()> {
        self.item.0.lock().await.title = title.to_owned();
        let emitter = SignalEmitter::new(&self.connection, ITEM_PATH)?;
        FakeItem::new_title(&emitter).await
    }

    pub async fn set_id(&self, id: &str) -> zbus::Result<()> {
        self.item.0.lock().await.id = id.to_owned();
        let emitter = SignalEmitter::new(&self.connection, ITEM_PATH)?;
        FakeItem::new_title(&emitter).await
    }

    pub async fn set_icon_name_silently(&self, icon_name: &str) {
        self.item.0.lock().await.icon_name = icon_name.to_owned();
    }

    pub async fn set_behaviour(&self, behaviour: ItemBehaviour) {
        self.item.0.lock().await.behaviour = behaviour;
    }

    pub async fn activate_calls(&self) -> u32 {
        self.item.0.lock().await.activate_calls
    }

    pub async fn context_calls(&self) -> u32 {
        self.item.0.lock().await.context_calls
    }

    pub async fn activation_tokens(&self) -> Vec<String> {
        self.item.0.lock().await.activation_tokens.clone()
    }
}
