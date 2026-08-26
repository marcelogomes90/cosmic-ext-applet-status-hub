pub mod icons;
pub mod menu_view;
pub mod message;
pub mod popup;
pub mod subscription;

use cosmic::Element;
use cosmic::app::{Core, Task};
use cosmic::applet::token::subscription::{
    TokenRequest, TokenUpdate, activation_token_subscription,
};
use cosmic::cctk::sctk::reexports::calloop;
use cosmic::iced::mouse::Interaction;
use cosmic::iced::platform_specific::shell::commands::popup::destroy_popup;
use cosmic::iced::{Length, Subscription, window};
use cosmic::widget::{icon, mouse_area, text};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::APP_ID;
use crate::applet::icons::IconCache;
use crate::applet::message::Message;
use crate::core::icons::IconKind;
use crate::core::menu::MenuModel;
use crate::core::model::{ItemAddress, TraySnapshot, WatcherState};
use crate::core::{CoreCommand, CoreHandle};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PopupState {
    #[default]
    Closed,
    Open {
        tray: window::Id,
        closing: bool,
    },
}

fn reconcile_surface_closed(id: window::Id, state: &mut PopupState) -> bool {
    match *state {
        PopupState::Open { tray, .. } if tray == id => {
            *state = PopupState::Closed;
            true
        }
        _ => false,
    }
}

#[derive(Clone, Debug)]
enum PendingTokenAction {
    Activate(ItemAddress),
    Menu {
        address: ItemAddress,
        id: i32,
        submenu: bool,
    },
}

pub struct StatusHub {
    core: Core,
    tray: CoreHandle,
    snapshot: Arc<TraySnapshot>,
    icons: RefCell<IconCache>,
    popup_state: PopupState,
    menu: Option<Arc<MenuModel>>,
    expanded: Vec<i32>,
    pending_menu: Option<ItemAddress>,
    token_tx: Option<calloop::channel::Sender<TokenRequest>>,
    next_token_request: u64,
    pending_tokens: HashMap<String, PendingTokenAction>,
}

impl StatusHub {
    fn item_icon_size(&self) -> u16 {
        self.core.applet.suggested_size(true).0
    }

    fn tray_padding() -> u16 {
        cosmic::theme::spacing().space_xxxs
    }

    fn button_size(&self) -> (u16, u16) {
        let (icon_width, icon_height) = self.core.applet.suggested_size(true);
        let (major_padding, minor_padding) = self.core.applet.suggested_padding(true);
        let (horizontal_padding, vertical_padding) = if self.core.applet.is_horizontal() {
            (major_padding, minor_padding)
        } else {
            (minor_padding, major_padding)
        };

        (
            icon_width.saturating_add(horizontal_padding.saturating_mul(2)),
            icon_height.saturating_add(vertical_padding.saturating_mul(2)),
        )
    }

    fn tray_geometry(&self) -> popup::GridGeometry {
        let (item_width, item_height) = self.button_size();

        popup::GridGeometry::calculate(
            self.snapshot.items.len(),
            u32::from(item_width),
            u32::from(item_height),
            self.core.applet.spacing,
            u32::from(Self::tray_padding()),
        )
    }

    fn item_button<'a>(&'a self, item: &'a crate::core::model::TrayItem) -> Element<'a, Message> {
        let size = self.item_icon_size();
        let handle = self
            .icons
            .borrow()
            .get(&item.address, item.generation, IconKind::Primary, size)
            .cloned();

        let button = match handle {
            Some(handle) => self.icon_button(handle, size),
            None => self
                .core
                .applet
                .button_from_element(text(item.label().to_owned()), true)
                .force_enabled(true),
        };

        let button: Element<'a, Message> = if self.is_selected(&item.address) {
            popup::selected_item(button)
        } else {
            button.into()
        };

        let address = item.address.clone();
        let mut area = mouse_area(button)
            .interaction(Interaction::Pointer)
            .on_press(Message::Activate(address.clone()))
            .on_middle_press(Message::SecondaryActivate(address.clone()));
        if item.menu_path.is_some() {
            area = area.on_right_press(Message::ContextMenu(address));
        }

        area.into()
    }

    fn icon_button<'a>(
        &self,
        handle: icon::Handle,
        size: u16,
    ) -> cosmic::widget::Button<'a, Message> {
        let size = f32::from(size);
        let symbolic = handle.symbolic;

        let glyph = cosmic::widget::icon(handle)
            .class(if symbolic {
                cosmic::theme::Svg::Custom(std::rc::Rc::new(|theme: &cosmic::Theme| {
                    cosmic::iced::widget::svg::Style {
                        color: Some(theme.cosmic().background(theme.transparent).on.into()),
                    }
                }))
            } else {
                cosmic::theme::Svg::default()
            })
            .width(Length::Fixed(size))
            .height(Length::Fixed(size));

        self.core
            .applet
            .button_from_element(glyph, true)
            .force_enabled(true)
    }

    fn is_selected(&self, address: &ItemAddress) -> bool {
        self.pending_menu.as_ref() == Some(address)
            || self
                .menu
                .as_ref()
                .is_some_and(|menu| &menu.owner == address)
    }

    fn popup_body(&self) -> Element<'_, Message> {
        if self.snapshot.items.is_empty() {
            let message = match &self.snapshot.watcher {
                WatcherState::Unavailable(_) => "No status notifier watcher is running",
                WatcherState::Connecting => "Connecting…",
                WatcherState::Connected => "No tray items",
            };
            return cosmic::applet::padded_control(text::body(message)).into();
        }

        self.icons
            .borrow_mut()
            .refresh(&self.snapshot, self.item_icon_size());
        let spacing = u16::try_from(self.core.applet.spacing).unwrap_or(0);
        popup::item_grid(
            self.snapshot
                .items
                .iter()
                .map(|item| self.item_button(item)),
            spacing,
        )
    }

    fn request_token(&mut self, action: PendingTokenAction) -> Task<Message> {
        let Some(token_tx) = self.token_tx.clone() else {
            return self.complete_token_action(action, None);
        };

        self.next_token_request = self.next_token_request.wrapping_add(1);
        let exec = format!("status-hub-action:{}", self.next_token_request);
        self.pending_tokens.insert(exec.clone(), action);
        if token_tx
            .send(TokenRequest {
                app_id: APP_ID.to_owned(),
                exec: exec.clone(),
            })
            .is_err()
        {
            let action = self
                .pending_tokens
                .remove(&exec)
                .expect("the pending token action was just inserted");
            return self.complete_token_action(action, None);
        }
        Task::none()
    }

    fn complete_token_action(
        &mut self,
        action: PendingTokenAction,
        token: Option<String>,
    ) -> Task<Message> {
        match action {
            PendingTokenAction::Activate(address) => {
                self.tray.send(CoreCommand::Primary { address, token });
                self.close_popup(true)
            }
            PendingTokenAction::Menu {
                address,
                id,
                submenu,
            } => {
                self.tray.send(CoreCommand::MenuClicked {
                    address,
                    id,
                    token,
                    close: !submenu,
                });
                if submenu {
                    if let Some(position) = self.expanded.iter().position(|open| *open == id) {
                        self.expanded.remove(position);
                    } else {
                        self.expanded.push(id);
                    }
                    Task::none()
                } else {
                    self.close_popup(false)
                }
            }
        }
    }

    fn on_menu(&mut self, menu: Option<Arc<MenuModel>>) -> Task<Message> {
        self.pending_menu = None;

        if self.popup_state == PopupState::Closed {
            if menu.is_some() {
                self.tray.send(CoreCommand::CloseMenu);
            }
            self.menu = None;
            self.expanded.clear();
            return Task::none();
        }

        let owner_changed = self
            .menu
            .as_ref()
            .zip(menu.as_ref())
            .is_some_and(|(old, new)| old.owner != new.owner);
        if owner_changed || menu.is_none() {
            self.expanded.clear();
        }
        self.menu = menu;

        cosmic::task::message(Message::Relayout)
    }

    fn close_popup(&mut self, close_menu: bool) -> Task<Message> {
        if close_menu && self.menu.is_some() {
            self.tray.send(CoreCommand::CloseMenu);
        }
        self.expanded.clear();
        self.menu = None;
        self.pending_menu = None;

        match self.popup_state {
            PopupState::Open {
                tray,
                closing: false,
            } => {
                self.popup_state = PopupState::Open {
                    tray,
                    closing: true,
                };
                destroy_popup(tray)
            }
            PopupState::Closed | PopupState::Open { closing: true, .. } => Task::none(),
        }
    }

    fn open_popup_surface(parent: window::Id, id: window::Id) -> Task<Message> {
        cosmic::surface::surface_task(cosmic::surface::action::app_popup::<Self>(
            |_| cosmic::surface::action::LiveSettings::default(),
            move |app| {
                let geometry = app.tray_geometry();
                let mut settings = app.core.applet.get_popup_settings(
                    parent,
                    id,
                    Some((u32::from(geometry.width), u32::from(geometry.height))),
                    None,
                    None,
                );
                settings.positioner.size_limits = popup::size_limits();
                settings
            },
            None,
        ))
    }
}

impl cosmic::Application for StatusHub {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = CoreHandle;
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn init(core: Core, tray: Self::Flags) -> (Self, Task<Message>) {
        let snapshot = tray.snapshot();
        (
            Self {
                core,
                tray,
                snapshot,
                icons: RefCell::new(IconCache::default()),
                popup_state: PopupState::Closed,
                menu: None,
                expanded: Vec::new(),
                pending_menu: None,
                token_tx: None,
                next_token_request: 0,
                pending_tokens: HashMap::new(),
            },
            Task::none(),
        )
    }

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            subscription::snapshots(&self.tray),
            subscription::menus(&self.tray),
            activation_token_subscription(0).map(Message::Token),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Relayout => Task::none(),

            Message::Snapshot(snapshot) => {
                let emptied = !self.snapshot.items.is_empty() && snapshot.items.is_empty();
                self.snapshot = snapshot;

                if emptied && self.popup_state != PopupState::Closed {
                    return self.close_popup(true);
                }
                Task::none()
            }

            Message::TogglePopup => {
                if self.popup_state != PopupState::Closed {
                    return self.close_popup(true);
                }

                let popup = window::Id::unique();
                self.popup_state = PopupState::Open {
                    tray: popup,
                    closing: false,
                };
                Self::open_popup_surface(
                    self.core
                        .main_window_id()
                        .expect("applet has a main window"),
                    popup,
                )
            }

            Message::SurfaceClosed(id) => {
                if reconcile_surface_closed(id, &mut self.popup_state) {
                    self.expanded.clear();
                    self.menu = None;
                    self.pending_menu = None;
                    self.tray.send(CoreCommand::CloseMenu);
                }
                Task::none()
            }

            Message::Activate(address) => self.request_token(PendingTokenAction::Activate(address)),

            Message::SecondaryActivate(address) => {
                self.tray.send(CoreCommand::Secondary { address });
                Task::none()
            }

            Message::ContextMenu(address) => {
                self.pending_menu = Some(address.clone());
                self.tray.send(CoreCommand::Context { address });
                Task::none()
            }

            Message::Menu(menu) => self.on_menu(menu),

            Message::MenuEntry { id, submenu } => match self.menu.as_ref() {
                Some(menu) => self.request_token(PendingTokenAction::Menu {
                    address: menu.owner.clone(),
                    id,
                    submenu,
                }),
                None => Task::none(),
            },

            Message::Token(update) => {
                match update {
                    TokenUpdate::Init(sender) => self.token_tx = Some(sender),
                    TokenUpdate::Finished => {
                        self.token_tx = None;
                        let pending = std::mem::take(&mut self.pending_tokens);
                        return Task::batch(
                            pending
                                .into_values()
                                .map(|action| self.complete_token_action(action, None)),
                        );
                    }
                    TokenUpdate::ActivationToken { token, exec } => {
                        if let Some(action) = self.pending_tokens.remove(&exec) {
                            return self.complete_token_action(action, token);
                        }
                    }
                }
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        if self.snapshot.items.is_empty() {
            return self
                .core
                .applet
                .autosize_window(cosmic::widget::space::horizontal().width(Length::Fixed(0.0)))
                .into();
        }

        let button = self
            .core
            .applet
            .icon_button(popup::panel_icon(
                self.core.applet.anchor,
                self.popup_state != PopupState::Closed,
            ))
            .on_press(Message::TogglePopup);

        self.core.applet.autosize_window(button).into()
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        if matches!(self.popup_state, PopupState::Open { tray, .. } if tray == id) {
            return popup::surface_container(
                self.popup_body(),
                self.menu
                    .as_ref()
                    .map(|menu| menu_view::view(menu, &self.expanded)),
                self.tray_geometry(),
                Self::tray_padding(),
                self.core.applet.anchor,
            );
        }
        text::body("").into()
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::SurfaceClosed(id))
    }
}

pub fn run(tray: CoreHandle) -> cosmic::iced::Result {
    cosmic::applet::run::<StatusHub>(tray)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_the_popup_leaves_no_surface() {
        let id = window::Id::unique();
        let mut state = PopupState::Open {
            tray: id,
            closing: false,
        };

        assert!(reconcile_surface_closed(id, &mut state));
        assert_eq!(state, PopupState::Closed);
    }

    #[test]
    fn an_unrelated_surface_does_not_change_popup_state() {
        let id = window::Id::unique();
        let mut state = PopupState::Open {
            tray: id,
            closing: false,
        };

        assert!(!reconcile_surface_closed(window::Id::unique(), &mut state));
        assert_eq!(
            state,
            PopupState::Open {
                tray: id,
                closing: false
            }
        );
    }
}
