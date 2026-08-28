use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};

pub const MENU_PATH: &str = "/MenuBar";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuBehaviour {
    #[default]
    Normal,
    MalformedProperty,
    Broken,
    Empty,
    Submenu,
    SlowAnnouncement,
}

#[derive(Debug, Default)]
struct State {
    behaviour: MenuBehaviour,
    revision: u32,
    events: Vec<MenuEvent>,
    about_to_show_calls: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEvent {
    pub id: i32,
    pub name: String,
    pub timestamp: u32,
}

#[derive(Clone, Debug, Default)]
pub struct FakeMenu(Arc<Mutex<State>>);

type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn props(pairs: &[(&str, Value<'static>)]) -> HashMap<String, OwnedValue> {
    pairs
        .iter()
        .map(|(key, value)| {
            (
                (*key).to_owned(),
                OwnedValue::try_from(value.clone()).expect("test property is convertible"),
            )
        })
        .collect()
}

fn child(id: i32, pairs: &[(&str, Value<'static>)]) -> OwnedValue {
    let node: Layout = (id, props(pairs), Vec::new());
    OwnedValue::try_from(Value::from(node)).expect("test layout node is convertible")
}

fn branch(id: i32, pairs: &[(&str, Value<'static>)], children: Vec<OwnedValue>) -> OwnedValue {
    let node: Layout = (id, props(pairs), children);
    OwnedValue::try_from(Value::from(node)).expect("test layout node is convertible")
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl FakeMenu {
    async fn get_layout(
        &self,
        _parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Layout)> {
        let state = self.0.lock().await;

        let children = match state.behaviour {
            MenuBehaviour::Broken => {
                return Err(zbus::fdo::Error::Failed("deliberately broken".into()));
            }
            MenuBehaviour::Empty => vec![child(1, &[("type", "separator".into())])],
            MenuBehaviour::Submenu => vec![branch(
                4,
                &[
                    ("label", "More".into()),
                    ("children-display", "submenu".into()),
                ],
                vec![child(5, &[("label", "Nested".into())])],
            )],
            MenuBehaviour::Normal | MenuBehaviour::SlowAnnouncement => vec![
                child(1, &[("label", "Open".into())]),
                child(2, &[("type", "separator".into())]),
                child(3, &[("label", "Quit".into())]),
            ],
            MenuBehaviour::MalformedProperty => vec![
                child(1, &[("label", "Open".into())]),
                child(
                    2,
                    &[
                        ("label", "Quit".into()),
                        ("shortcut", Value::from(vec![vec!["Control".to_owned()]])),
                    ],
                ),
            ],
        };

        Ok((
            state.revision,
            (
                0,
                props(&[("children-display", "submenu".into())]),
                children,
            ),
        ))
    }

    async fn event(&self, id: i32, event_id: &str, _data: Value<'_>, timestamp: u32) {
        self.0.lock().await.events.push(MenuEvent {
            id,
            name: event_id.to_owned(),
            timestamp,
        });
    }

    async fn about_to_show(&self, _id: i32) -> bool {
        let slow = {
            let mut state = self.0.lock().await;
            state.about_to_show_calls += 1;
            state.behaviour == MenuBehaviour::SlowAnnouncement
        };
        if slow {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        false
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

impl FakeMenu {
    pub async fn serve(
        connection: &zbus::Connection,
        behaviour: MenuBehaviour,
    ) -> zbus::Result<Self> {
        let menu = Self(Arc::new(Mutex::new(State {
            behaviour,
            revision: 1,
            ..State::default()
        })));
        connection
            .object_server()
            .at(MENU_PATH, menu.clone())
            .await?;
        Ok(menu)
    }

    pub async fn events(&self) -> Vec<(i32, String)> {
        self.0
            .lock()
            .await
            .events
            .iter()
            .map(|event| (event.id, event.name.clone()))
            .collect()
    }

    pub async fn event_details(&self) -> Vec<MenuEvent> {
        self.0.lock().await.events.clone()
    }

    pub async fn about_to_show_calls(&self) -> u32 {
        self.0.lock().await.about_to_show_calls
    }

    pub async fn announce_change(
        &self,
        connection: &zbus::Connection,
        behaviour: MenuBehaviour,
    ) -> zbus::Result<()> {
        let revision = {
            let mut state = self.0.lock().await;
            state.behaviour = behaviour;
            state.revision += 1;
            state.revision
        };
        let emitter = SignalEmitter::new(connection, MENU_PATH)?;
        Self::layout_updated(&emitter, revision, 0).await
    }
}
