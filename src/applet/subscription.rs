use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cosmic::iced::Subscription;
use futures::Stream;

use crate::applet::message::Message;
use crate::core::CoreHandle;

pub fn menus(handle: &CoreHandle) -> Subscription<Message> {
    Subscription::run_with(MenuSource(handle.clone()), |MenuSource(handle)| {
        menu_stream(handle.clone())
    })
}

pub struct Source(CoreHandle);

impl Hash for Source {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "cosmic-status-hub-snapshots".hash(state);
    }
}

pub struct MenuSource(CoreHandle);

impl Hash for MenuSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "cosmic-status-hub-menu".hash(state);
    }
}

#[allow(clippy::needless_pass_by_value)]
fn menu_stream(handle: CoreHandle) -> impl futures::Stream<Item = Message> + Send + 'static {
    futures::stream::unfold(
        (handle.subscribe_menu(), true),
        |(mut receiver, first)| async move {
            if !first && receiver.changed().await.is_err() {
                return None;
            }
            let menu = receiver.borrow_and_update().clone();
            Some((Message::Menu(menu), (receiver, false)))
        },
    )
}

pub fn snapshots(handle: &CoreHandle) -> Subscription<Message> {
    Subscription::run_with(Source(handle.clone()), |Source(handle)| {
        stream(handle.clone())
    })
}

#[allow(clippy::needless_pass_by_value)]
fn stream(handle: CoreHandle) -> impl Stream<Item = Message> + Send + 'static {
    futures::stream::unfold(
        (handle.subscribe(), true),
        |(mut receiver, first)| async move {
            if !first && receiver.changed().await.is_err() {
                return None;
            }
            let snapshot = Arc::clone(&receiver.borrow_and_update());
            Some((Message::Snapshot(snapshot), (receiver, false)))
        },
    )
}

pub fn pins() -> Subscription<Message> {
    cosmic::cosmic_config::config_subscription::<_, crate::applet::pins::Pins>(
        "cosmic-status-hub-pins",
        crate::APP_ID.into(),
        crate::applet::pins::CONFIG_VERSION,
    )
    .map(|update| Message::PinsChanged(update.config))
}

pub fn order() -> Subscription<Message> {
    cosmic::cosmic_config::config_subscription::<_, crate::applet::order::Order>(
        "cosmic-status-hub-order",
        crate::APP_ID.into(),
        crate::applet::pins::CONFIG_VERSION,
    )
    .map(|update| Message::OrderChanged(update.config))
}
