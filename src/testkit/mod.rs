#![allow(clippy::unused_self, clippy::used_underscore_binding)]

mod bus;
mod item;
mod menu;
mod watcher;

pub use bus::PrivateBus;
pub use item::{FakeItem, ItemBehaviour, ItemHandle};
pub use menu::{FakeMenu, MENU_PATH, MenuBehaviour};
pub use watcher::{FakeCosmicWatcher, FakeWatcher};

pub const ITEM_PATH: &str = "/StatusNotifierItem";
