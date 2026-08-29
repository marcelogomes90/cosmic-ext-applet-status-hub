pub mod icons;
pub mod menu_view;
pub mod message;
pub mod pins;
pub mod popup;
pub mod subscription;

use cosmic::Element;
use cosmic::app::{Core, Task};
use cosmic::applet::token::subscription::{
    TokenRequest, TokenUpdate, activation_token_subscription,
};
use cosmic::cctk::sctk::reexports::calloop;
use cosmic::iced::advanced::text::{Ellipsize, EllipsizeHeightLimit};
use cosmic::iced::mouse::Interaction;
use cosmic::iced::platform_specific::runtime::wayland::popup::SctkPositioner;
use cosmic::iced::platform_specific::shell::commands::popup::destroy_popup;
use cosmic::iced::{Length, Subscription, window};
use cosmic::widget::button::Catalog as _;
use cosmic::widget::{icon, mouse_area, text};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::APP_ID;
use crate::applet::icons::IconCache;
use crate::applet::message::Message;
use crate::core::icons::IconKind;
use crate::core::menu::MenuModel;
use crate::core::model::{ItemAddress, TraySnapshot, WatcherState};
use crate::core::{CoreCommand, CoreHandle};
use crate::fl;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PopupState {
    #[default]
    Closed,
    Open {
        tray: window::Id,
        closing: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PopupBody {
    #[default]
    Items,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuOrigin {
    Hub,
    Panel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClosedSurface {
    Unknown,
    Tray,
}

fn reconcile_surface_closed(id: window::Id, state: &mut PopupState) -> ClosedSurface {
    let PopupState::Open { tray, .. } = *state else {
        return ClosedSurface::Unknown;
    };

    if tray == id {
        *state = PopupState::Closed;
        return ClosedSurface::Tray;
    }

    ClosedSurface::Unknown
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

fn detect_wing(panel: &str) -> popup::Wing {
    use cosmic::cosmic_config::{Config, ConfigGet};

    let Ok(config) = Config::new(&format!("com.system76.CosmicPanel.{panel}"), 1) else {
        return popup::Wing::default();
    };

    if let Ok(Some((start, end))) =
        config.get::<Option<(Vec<String>, Vec<String>)>>("plugins_wings")
    {
        if start.iter().any(|plugin| plugin == APP_ID) {
            return popup::Wing::Start;
        }
        if end.iter().any(|plugin| plugin == APP_ID) {
            return popup::Wing::End;
        }
    }

    if let Ok(Some(center)) = config.get::<Option<Vec<String>>>("plugins_center")
        && center.iter().any(|plugin| plugin == APP_ID)
    {
        return popup::Wing::Center;
    }

    popup::Wing::default()
}

fn accent_icon_style(
    mut style: cosmic::widget::button::Style,
    theme: &cosmic::Theme,
) -> cosmic::widget::button::Style {
    style.icon_color = Some(theme.cosmic().accent.base.into());
    style
}

fn accent_icon_button() -> cosmic::theme::Button {
    cosmic::theme::Button::Custom {
        active: Box::new(|focused, theme| {
            accent_icon_style(
                theme.active(focused, false, &cosmic::theme::Button::Icon),
                theme,
            )
        }),
        disabled: Box::new(|theme| theme.disabled(&cosmic::theme::Button::Icon)),
        hovered: Box::new(|focused, theme| {
            accent_icon_style(
                theme.hovered(focused, false, &cosmic::theme::Button::Icon),
                theme,
            )
        }),
        pressed: Box::new(|focused, theme| {
            accent_icon_style(
                theme.pressed(focused, false, &cosmic::theme::Button::Icon),
                theme,
            )
        }),
    }
}

pub struct StatusHub {
    core: Core,
    tray: CoreHandle,
    snapshot: Arc<TraySnapshot>,
    icons: RefCell<IconCache>,
    popup_state: PopupState,
    body: PopupBody,
    pins: pins::Pins,
    draft: Option<pins::Pins>,
    wing: popup::Wing,
    pin_store: pins::PinStore,
    menu: Option<Arc<MenuModel>>,
    menu_origin: MenuOrigin,
    panel_menu: Option<window::Id>,
    expanded: Vec<i32>,
    pending_menu: Option<ItemAddress>,
    token_tx: Option<calloop::channel::Sender<TokenRequest>>,
    next_token_request: u64,
    pending_tokens: HashMap<String, PendingTokenAction>,
    icon_retry_attempt: usize,
    icon_retry_nonce: u64,
    pending_icon_retry: Option<u64>,
}

const ICON_RETRY_DELAYS: [Duration; 6] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(8),
    Duration::from_secs(20),
    Duration::from_secs(45),
];
impl StatusHub {
    fn item_icon_size(&self) -> u16 {
        self.core.applet.suggested_size(true).0
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
        let (_, popup_items) = self.partition_items(&self.pins);

        popup::GridGeometry::calculate(
            popup_items.len(),
            u32::from(item_width),
            u32::from(item_height),
            self.core.applet.spacing,
            u32::from(popup::CONTENT_PADDING),
            u32::from(popup::HEADER_PADDING),
        )
    }

    fn partition_items(
        &self,
        pins: &pins::Pins,
    ) -> (
        Vec<&crate::core::model::TrayItem>,
        Vec<&crate::core::model::TrayItem>,
    ) {
        pins::partition_for_panel(
            &self.snapshot.items,
            pins,
            popup::panel_item_capacity(
                self.core.applet.suggested_bounds,
                self.button_size(),
                self.core.applet.is_horizontal(),
            ),
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
            Some(handle) => self
                .core
                .applet
                .button_from_element(Self::icon_glyph(handle, size), true)
                .force_enabled(true),
            None => self
                .core
                .applet
                .button_from_element(text(item.label().to_owned()), true)
                .force_enabled(true),
        };

        let address = item.address.clone();

        let button: Element<'a, Message> = if self.is_selected(&item.address) {
            popup::selected_item(button)
        } else {
            button.into()
        };

        let area = mouse_area(button)
            .interaction(Interaction::Pointer)
            .on_press(Message::Activate(address.clone()))
            .on_middle_press(Message::SecondaryActivate(address.clone()))
            .on_right_press(Message::ContextMenu(address));

        area.into()
    }

    fn icon_glyph<'a>(handle: icon::Handle, size: u16) -> Element<'a, Message> {
        let size = f32::from(size);
        let symbolic = handle.symbolic;

        cosmic::widget::icon(handle)
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
            .height(Length::Fixed(size))
            .into()
    }

    fn is_selected(&self, address: &ItemAddress) -> bool {
        self.pending_menu.as_ref() == Some(address)
            || self
                .menu
                .as_ref()
                .is_some_and(|menu| &menu.owner == address)
    }

    fn hub_layout(&self) -> popup::HubLayout {
        let body = match self.body {
            PopupBody::Items => self.items_height(),
            PopupBody::Settings => popup::settings_body_height(
                self.snapshot.items.len(),
                popup::SETTINGS_ROW,
                cosmic::theme::spacing().space_xxs,
                popup::HEADER_PADDING,
            ),
        };

        let header = popup::header_height(popup::HEADER_CONTROL, popup::HEADER_PADDING);
        let separator = popup::separator_height();
        if self.menu.is_some() && self.menu_origin == MenuOrigin::Hub {
            popup::HubLayout::with_menu(header, separator, body, popup::menu_separator_height())
        } else {
            popup::HubLayout::new(header, separator, body)
        }
    }

    fn items_height(&self) -> u16 {
        let geometry = self.tray_geometry();
        let (_, popup_items) = self.partition_items(&self.pins);

        if popup_items.is_empty() {
            popup::notice_height(geometry.height)
        } else {
            geometry.height
        }
    }

    fn header(&self) -> Element<'_, Message> {
        let showing_settings = self.body == PopupBody::Settings;
        let control = f32::from(popup::HEADER_CONTROL);

        let action: Element<'_, Message> = if showing_settings {
            cosmic::widget::button::text(fl!("save"))
                .height(Length::Fixed(control))
                .on_press(Message::SaveSettings)
                .into()
        } else {
            let settings = cosmic::widget::button::icon(
                icon::from_name(popup::SETTINGS_ICON)
                    .size(popup::HEADER_ICON)
                    .symbolic(true),
            )
            .class(accent_icon_button())
            .width(Length::Fixed(control))
            .height(Length::Fixed(control));
            if self.snapshot.items.is_empty() {
                settings.into()
            } else {
                settings.on_press(Message::OpenSettings).into()
            }
        };

        let title = if showing_settings {
            fl!("settings")
        } else {
            fl!("app-title")
        };

        popup::header(
            title,
            action,
            popup::header_height(popup::HEADER_CONTROL, popup::HEADER_PADDING),
            popup::HEADER_PADDING,
            self.menu.is_some().then_some(Message::DismissMenu),
        )
    }

    fn popup_body(&self) -> Element<'_, Message> {
        match self.body {
            PopupBody::Items => self.items_body(),
            PopupBody::Settings => self.settings_body(),
        }
    }

    fn items_body(&self) -> Element<'_, Message> {
        let (_, popup_items) = self.partition_items(&self.pins);

        if popup_items.is_empty() {
            let message = if self.snapshot.items.is_empty() {
                match &self.snapshot.watcher {
                    WatcherState::Unavailable(_) => fl!("no-watcher"),
                    WatcherState::Connecting => fl!("connecting"),
                    WatcherState::Connected => fl!("empty-state"),
                }
            } else {
                fl!("empty-state")
            };
            return popup::notice(message, self.items_height());
        }

        self.icons
            .borrow_mut()
            .refresh(&self.snapshot, self.item_icon_size(), false);
        let spacing = u16::try_from(self.core.applet.spacing).unwrap_or(0);
        popup::item_grid(
            popup_items.into_iter().map(|item| self.item_button(item)),
            spacing,
            self.tray_geometry().height,
        )
    }

    fn settings_body(&self) -> Element<'_, Message> {
        self.icons
            .borrow_mut()
            .refresh(&self.snapshot, self.item_icon_size(), false);
        let spacing = cosmic::theme::spacing().space_xxs;

        popup::settings_list(
            self.snapshot
                .items
                .iter()
                .map(|item| self.settings_row(item)),
            popup::settings_body_height(
                self.snapshot.items.len(),
                popup::SETTINGS_ROW,
                spacing,
                popup::HEADER_PADDING,
            ),
            spacing,
            popup::HEADER_PADDING,
        )
    }

    fn settings_row<'a>(&'a self, item: &'a crate::core::model::TrayItem) -> Element<'a, Message> {
        let spacing = cosmic::theme::spacing();
        let key = item.key.clone();
        let pinned = self
            .draft
            .as_ref()
            .unwrap_or(&self.pins)
            .contains(&item.key);

        let label = text::body(item.label().to_owned())
            .width(Length::Fill)
            .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));

        let size = self.item_icon_size().min(popup::SETTINGS_ROW);
        let handle = self
            .icons
            .borrow()
            .get(
                &item.address,
                item.generation,
                IconKind::Primary,
                self.item_icon_size(),
            )
            .cloned()
            .unwrap_or_else(|| icon::from_name("application-default").size(size).handle());

        let row = cosmic::widget::row::with_children(vec![
            Self::icon_glyph(handle, size),
            label.into(),
            cosmic::widget::toggler(pinned)
                .on_toggle(move |_| Message::TogglePin(key.clone()))
                .into(),
        ])
        .align_y(cosmic::iced::Alignment::Center)
        .spacing(spacing.space_xs);

        cosmic::widget::container(row)
            .width(Length::Fill)
            .height(Length::Fixed(f32::from(popup::SETTINGS_ROW)))
            .align_y(cosmic::iced::Alignment::Center)
            .into()
    }

    fn request_token(&mut self, action: PendingTokenAction) -> Task<Message> {
        let Some(token_tx) = self.token_tx.clone() else {
            tracing::debug!("no activation token channel is available");
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
            tracing::warn!("the activation token channel is closed");
            return self.complete_token_action(action, None);
        }
        Task::none()
    }

    fn on_token(&mut self, update: TokenUpdate) -> Task<Message> {
        match update {
            TokenUpdate::Init(sender) => {
                self.token_tx = Some(sender);
                Task::none()
            }
            TokenUpdate::Finished => {
                tracing::warn!("the activation token connection ended");
                self.token_tx = None;
                let pending = std::mem::take(&mut self.pending_tokens);
                Task::batch(
                    pending
                        .into_values()
                        .map(|action| self.complete_token_action(action, None)),
                )
            }
            TokenUpdate::ActivationToken { token, exec } => {
                tracing::info!(request = %exec, token = token.is_some(), "activation token");
                match self.pending_tokens.remove(&exec) {
                    Some(action) => self.complete_token_action(action, token),
                    None => Task::none(),
                }
            }
        }
    }

    fn cancel_icon_retry(&mut self) {
        self.icon_retry_nonce = self.icon_retry_nonce.wrapping_add(1);
        self.pending_icon_retry = None;
        self.icon_retry_attempt = 0;
    }

    fn schedule_icon_retry(&mut self) -> Task<Message> {
        let Some(&delay) = ICON_RETRY_DELAYS.get(self.icon_retry_attempt) else {
            self.pending_icon_retry = None;
            return Task::none();
        };
        self.icon_retry_attempt += 1;
        self.icon_retry_nonce = self.icon_retry_nonce.wrapping_add(1);
        let nonce = self.icon_retry_nonce;
        self.pending_icon_retry = Some(nonce);
        cosmic::task::future(async move {
            tokio::time::sleep(delay).await;
            Message::RetryIcons(nonce)
        })
    }

    fn refresh_icons(&mut self, retry_fallbacks: bool, restart: bool) -> Task<Message> {
        let unresolved =
            self.icons
                .borrow_mut()
                .refresh(&self.snapshot, self.item_icon_size(), retry_fallbacks);
        if !unresolved {
            self.cancel_icon_retry();
            return Task::none();
        }
        if restart {
            self.cancel_icon_retry();
        }
        self.schedule_icon_retry()
    }

    fn on_snapshot(&mut self, snapshot: Arc<TraySnapshot>) -> Task<Message> {
        let emptied = !self.snapshot.items.is_empty() && snapshot.items.is_empty();
        let slots_before = self.panel_slots();
        let active_menu = self
            .menu
            .as_ref()
            .map(|menu| menu.owner.clone())
            .or_else(|| self.pending_menu.clone());
        self.snapshot = snapshot;

        let icon_task = self.refresh_icons(false, true);
        let slots_changed = self.panel_slots() != slots_before;
        let active_disappeared = active_menu.is_some_and(|address| {
            !self
                .snapshot
                .items
                .iter()
                .any(|item| item.address == address)
        });
        let menu_task = if active_disappeared || (slots_changed && self.panel_menu.is_some()) {
            self.forget_menu();
            self.menu_origin = MenuOrigin::Hub;
            self.close_panel_menu()
        } else {
            Task::none()
        };

        if emptied && self.popup_state != PopupState::Closed {
            return Task::batch([icon_task, menu_task, self.close_popup(true)]);
        }
        Task::batch([icon_task, menu_task])
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
                } else if self.menu_origin == MenuOrigin::Panel {
                    self.menu = None;
                    self.expanded.clear();
                    self.menu_origin = MenuOrigin::Hub;
                    self.close_panel_menu()
                } else {
                    self.close_popup(false)
                }
            }
        }
    }

    fn on_panel_menu(&mut self, menu: Option<Arc<MenuModel>>) -> Task<Message> {
        let owner_changed = self
            .menu
            .as_ref()
            .zip(menu.as_ref())
            .is_some_and(|(old, new)| old.owner != new.owner);
        if owner_changed || menu.is_none() {
            self.expanded.clear();
        }

        let slot = menu
            .as_ref()
            .and_then(|model| self.panel_slot_of(&model.owner));
        let showing = self.panel_menu.is_some();
        self.menu = menu;

        let Some(slot) = slot else {
            self.menu = None;
            self.menu_origin = MenuOrigin::Hub;
            return self.close_panel_menu();
        };

        if showing && !owner_changed {
            return cosmic::task::message(Message::Relayout);
        }

        let closed = self.close_panel_menu();
        closed.chain(self.open_panel_menu(slot))
    }

    fn on_menu(&mut self, menu: Option<Arc<MenuModel>>) -> Task<Message> {
        self.pending_menu = None;

        if self.menu_origin == MenuOrigin::Panel {
            return self.on_panel_menu(menu);
        }

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

    fn on_toggle_popup(&mut self) -> Task<Message> {
        if self.popup_state != PopupState::Closed {
            return self.close_popup(true);
        }

        let closed_panel_menu = self.close_panel_menu();
        self.menu_origin = MenuOrigin::Hub;
        self.menu = None;
        self.pending_menu = None;
        self.expanded.clear();

        let popup = window::Id::unique();
        self.body = PopupBody::Items;
        self.popup_state = PopupState::Open {
            tray: popup,
            closing: false,
        };

        let parent = self
            .core
            .main_window_id()
            .expect("applet has a main window");
        closed_panel_menu.chain(Self::open_popup_surface(parent, popup))
    }

    fn on_surface_closed(&mut self, id: window::Id) -> Task<Message> {
        if self.panel_menu == Some(id) {
            self.panel_menu = None;
            self.menu_origin = MenuOrigin::Hub;
            self.forget_menu();
            return Task::none();
        }

        match reconcile_surface_closed(id, &mut self.popup_state) {
            ClosedSurface::Tray => {
                self.body = PopupBody::Items;
                self.draft = None;
                self.forget_menu();
            }
            ClosedSurface::Unknown => {}
        }
        Task::none()
    }

    fn on_open_settings(&mut self) -> Task<Message> {
        self.body = PopupBody::Settings;
        self.draft = Some(self.pins.clone());
        self.forget_menu();
        cosmic::task::message(Message::Relayout)
    }

    fn on_save_settings(&mut self) -> Task<Message> {
        self.body = PopupBody::Items;
        let draft = self.draft.take();
        self.forget_menu();

        if let Some(draft) = draft
            && draft != self.pins
        {
            self.pin_store.save(&draft);
            self.pins = draft;
        }

        self.close_popup(true)
    }

    fn forget_menu(&mut self) {
        self.expanded.clear();
        self.menu = None;
        self.pending_menu = None;
        self.tray.send(CoreCommand::CloseMenu);
    }

    fn close_panel_menu(&mut self) -> Task<Message> {
        match self.panel_menu.take() {
            Some(menu) => destroy_popup(menu),
            None => Task::none(),
        }
    }

    fn open_panel_menu(&mut self, slot: usize) -> Task<Message> {
        let Some(parent) = self.core.main_window_id() else {
            return Task::none();
        };

        let menu = window::Id::unique();
        self.panel_menu = Some(menu);

        cosmic::surface::surface_task(cosmic::surface::action::app_popup::<Self>(
            |_| cosmic::surface::action::LiveSettings::default(),
            move |app| {
                let button = app.button_size();
                let horizontal = app.core.applet.is_horizontal();
                let mut settings = app.core.applet.get_popup_settings(
                    parent,
                    menu,
                    Some((u32::from(popup::SURFACE_WIDTH), 1)),
                    None,
                    None,
                );
                settings.positioner.anchor_rect = popup::panel_slot_rect(slot, button, horizontal);
                settings.positioner.size_limits = popup::size_limits();
                settings
            },
            None,
        ))
    }

    fn panel_slot_of(&self, address: &ItemAddress) -> Option<usize> {
        let (pinned, _) = self.partition_items(&self.pins);
        pinned
            .iter()
            .position(|item| &item.address == address)
            .map(|index| popup::pinned_slot(self.wing, index))
    }

    fn panel_slots(&self) -> usize {
        let (panel, _) = self.partition_items(&self.pins);
        panel.len() + 1
    }

    fn apply_pins(&mut self, pins: pins::Pins) -> Task<Message> {
        if self.pins == pins {
            return Task::none();
        }

        self.pins = pins;

        let close_menu = if self.menu.is_some() || self.pending_menu.is_some() {
            self.forget_menu();
            self.menu_origin = MenuOrigin::Hub;
            self.close_panel_menu()
        } else {
            Task::none()
        };
        close_menu.chain(cosmic::task::message(Message::Relayout))
    }

    fn hub_positioner(&self, parent: window::Id, id: window::Id) -> SctkPositioner {
        let height = self.hub_layout().height();
        let mut settings = self.core.applet.get_popup_settings(
            parent,
            id,
            Some((u32::from(popup::SURFACE_WIDTH), u32::from(height))),
            None,
            None,
        );
        settings.positioner.anchor_rect = popup::panel_slot_rect(
            popup::hub_slot(self.wing, self.panel_slots()),
            self.button_size(),
            self.core.applet.is_horizontal(),
        );
        settings.positioner.reactive = false;
        settings.positioner.size_limits = popup::size_limits();
        settings.positioner
    }

    fn open_popup_surface(parent: window::Id, id: window::Id) -> Task<Message> {
        cosmic::surface::surface_task(cosmic::surface::action::app_popup::<Self>(
            |_| cosmic::surface::action::LiveSettings::default(),
            move |app| {
                let mut settings = app
                    .core
                    .applet
                    .get_popup_settings(parent, id, None, None, None);
                settings.positioner = app.hub_positioner(parent, id);
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
        let wing = detect_wing(&core.applet.panel_type.to_string());
        tracing::info!(?wing, "panel placement");
        let pin_store = pins::PinStore::open(APP_ID);
        let pins = pin_store.load();
        (
            Self {
                core,
                tray,
                snapshot,
                icons: RefCell::new(IconCache::default()),
                popup_state: PopupState::Closed,
                body: PopupBody::Items,
                pins,
                draft: None,
                wing,
                pin_store,
                menu: None,
                menu_origin: MenuOrigin::Hub,
                panel_menu: None,
                expanded: Vec::new(),
                pending_menu: None,
                token_tx: None,
                next_token_request: 0,
                pending_tokens: HashMap::new(),
                icon_retry_attempt: 0,
                icon_retry_nonce: 0,
                pending_icon_retry: None,
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
            subscription::pins(),
            activation_token_subscription(0).map(Message::Token),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Relayout => Task::none(),

            Message::Snapshot(snapshot) => self.on_snapshot(snapshot),

            Message::TogglePopup => self.on_toggle_popup(),

            Message::SurfaceClosed(id) => self.on_surface_closed(id),

            Message::Activate(address) => {
                tracing::info!(item = %address, "primary click");
                self.request_token(PendingTokenAction::Activate(address))
            }

            Message::SecondaryActivate(address) => {
                self.tray.send(CoreCommand::Secondary { address });
                Task::none()
            }

            Message::ContextMenu(address) => {
                self.menu_origin = if self.panel_slot_of(&address).is_some() {
                    MenuOrigin::Panel
                } else {
                    MenuOrigin::Hub
                };
                self.pending_menu = self
                    .snapshot
                    .items
                    .iter()
                    .find(|item| item.address == address)
                    .and_then(|item| item.menu_path.as_ref().map(|_| address.clone()));
                self.tray.send(CoreCommand::Context { address });
                Task::none()
            }

            Message::RetryIcons(nonce) => {
                if self.pending_icon_retry != Some(nonce) {
                    return Task::none();
                }
                self.pending_icon_retry = None;
                self.refresh_icons(true, false)
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

            Message::OpenSettings => self.on_open_settings(),

            Message::SaveSettings => self.on_save_settings(),

            Message::PinsChanged(pins) => self.apply_pins(pins),

            Message::DismissMenu => {
                if self.menu.is_none() {
                    return Task::none();
                }
                self.forget_menu();
                cosmic::task::message(Message::Relayout)
            }

            Message::TogglePin(key) => {
                let Some(draft) = self.draft.as_mut() else {
                    return Task::none();
                };
                if !draft.toggle(&key) {
                    return Task::none();
                }
                cosmic::task::message(Message::Relayout)
            }

            Message::Token(update) => self.on_token(update),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let horizontal = self.core.applet.is_horizontal();

        let hub = self
            .core
            .applet
            .icon_button(popup::PANEL_ICON)
            .on_press(Message::TogglePopup);

        let (pinned, _) = self.partition_items(&self.pins);
        if pinned.is_empty() {
            return popup::panel_surface(hub, self.core.applet.suggested_bounds, horizontal);
        }

        self.icons
            .borrow_mut()
            .refresh(&self.snapshot, self.item_icon_size(), false);

        let hub: Element<'_, Message> = hub.into();
        let buttons = pinned.into_iter().map(|item| self.item_button(item));
        let slots: Vec<Element<'_, Message>> = if popup::hub_leads(self.wing) {
            std::iter::once(hub).chain(buttons).collect()
        } else {
            buttons.chain(std::iter::once(hub)).collect()
        };

        let strip: Element<'_, Message> = if horizontal {
            cosmic::widget::row::with_children(slots)
                .align_y(cosmic::iced::Alignment::Center)
                .into()
        } else {
            cosmic::widget::column::with_children(slots)
                .align_x(cosmic::iced::Alignment::Center)
                .into()
        };

        popup::panel_surface(strip, self.core.applet.suggested_bounds, horizontal)
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        if self.panel_menu == Some(id) {
            return match self.menu.as_ref() {
                Some(model) => popup::menu_surface(menu_view::view(model, &self.expanded)),
                None => text::body("").into(),
            };
        }

        let PopupState::Open { tray, .. } = self.popup_state else {
            return text::body("").into();
        };

        if tray == id {
            let body = if self.menu.is_some() {
                mouse_area(self.popup_body())
                    .on_press(Message::DismissMenu)
                    .into()
            } else {
                self.popup_body()
            };
            let menu = self
                .menu
                .as_ref()
                .map(|model| menu_view::view(model, &self.expanded));
            return popup::hub_surface(
                self.header(),
                body,
                menu,
                self.hub_layout(),
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

    fn open(tray: window::Id) -> PopupState {
        PopupState::Open {
            tray,
            closing: false,
        }
    }

    #[test]
    fn closing_the_popup_leaves_no_surface() {
        let tray = window::Id::unique();
        let mut state = open(tray);

        assert_eq!(
            reconcile_surface_closed(tray, &mut state),
            ClosedSurface::Tray
        );
        assert_eq!(state, PopupState::Closed);
    }

    #[test]
    fn an_unrelated_surface_does_not_change_popup_state() {
        let tray = window::Id::unique();
        let mut state = open(tray);

        assert_eq!(
            reconcile_surface_closed(window::Id::unique(), &mut state),
            ClosedSurface::Unknown
        );
        assert_eq!(state, open(tray));
    }
}
