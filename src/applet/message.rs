use std::sync::Arc;

use cosmic::applet::token::subscription::TokenUpdate;
use cosmic::iced::window;

use crate::core::menu::MenuModel;
use crate::core::model::{ItemAddress, ItemKey, TraySnapshot};

#[derive(Clone, Debug)]
pub enum Message {
    Snapshot(Arc<TraySnapshot>),
    TogglePopup,
    SurfaceClosed(window::Id),
    Activate(ItemAddress),
    SecondaryActivate(ItemAddress),
    ContextMenu(ItemAddress),
    RetryIcons(u64),
    Menu(Option<Arc<MenuModel>>),
    MenuEntry { id: i32, submenu: bool },
    OpenSettings,
    SaveSettings,
    DismissMenu,
    PinsChanged(crate::applet::pins::Pins),
    TogglePin(ItemKey),
    Token(TokenUpdate),
    Relayout,
}
