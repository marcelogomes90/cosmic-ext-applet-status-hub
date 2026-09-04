use std::sync::Arc;

use cosmic::iced::window;

use crate::applet::wayland::WaylandUpdate;
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
    AppearanceChanged(crate::applet::appearance::Appearance),
    DismissMenu,
    PinsChanged(crate::applet::pins::Pins),
    OrderChanged(crate::applet::order::Order),
    TogglePin(ItemKey),
    ToggleColourIcons(bool),
    DragStart(ItemKey),
    DragOver(ItemKey),
    DragEnd,
    Wayland(WaylandUpdate),
    Relayout,
}
