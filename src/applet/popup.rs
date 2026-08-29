use std::sync::LazyLock;

use cosmic::Element;
use cosmic::applet::cosmic_panel_config::PanelAnchor;
use cosmic::iced::advanced::text::{Ellipsize, EllipsizeHeightLimit};
use cosmic::iced::{Alignment, Border, Color, Length, Limits, Rectangle, Shadow, Size};
use cosmic::widget::{autosize, container, divider, flex_row, mouse_area, scrollable, text};

pub const SURFACE_WIDTH: u16 = 360;
const MAX_HEIGHT: u16 = 1080;

pub const HEADER_CONTROL: u16 = 32;
pub const HEADER_ICON: u16 = 16;
pub const SETTINGS_ROW: u16 = 36;
pub const PANEL_ICON: &str = "view-more-horizontal-symbolic";
pub const SETTINGS_ICON: &str = "emblem-system-symbolic";
pub const CONTENT_PADDING: u16 = 16;
pub const HEADER_PADDING: u16 = 12;
pub const NOTICE_PADDING: u16 = HEADER_PADDING;

static TRAY_AUTOSIZE_ID: LazyLock<cosmic::iced::id::Id> =
    LazyLock::new(|| cosmic::iced::id::Id::new("cosmic-status-hub-tray-popup"));
static MENU_AUTOSIZE_ID: LazyLock<cosmic::iced::id::Id> =
    LazyLock::new(|| cosmic::iced::id::Id::new("cosmic-status-hub-menu-popup"));
static PANEL_AUTOSIZE_ID: LazyLock<cosmic::iced::id::Id> =
    LazyLock::new(|| cosmic::iced::id::Id::new("cosmic-status-hub-panel"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridGeometry {
    pub height: u16,
    pub columns: usize,
    pub rows: usize,
}

impl GridGeometry {
    pub fn calculate(
        item_count: usize,
        item_width: u32,
        item_height: u32,
        spacing: u32,
        horizontal_padding: u32,
        vertical_padding: u32,
    ) -> Self {
        let usable_width =
            u32::from(SURFACE_WIDTH).saturating_sub(horizontal_padding.saturating_mul(2));
        let columns_at_limit = usable_width
            .saturating_add(spacing)
            .checked_div(item_width.saturating_add(spacing).max(1))
            .unwrap_or(1)
            .max(1);
        let columns_at_limit = usize::try_from(columns_at_limit).unwrap_or(usize::MAX);
        let columns = item_count.clamp(1, columns_at_limit);
        let rows = item_count.max(1).div_ceil(columns);
        let rows_u32 = u32::try_from(rows).unwrap_or(u32::MAX);
        let content_height = rows_u32
            .saturating_mul(item_height)
            .saturating_add(rows_u32.saturating_sub(1).saturating_mul(spacing));
        let height = content_height
            .saturating_add(vertical_padding.saturating_mul(2))
            .clamp(1, u32::from(MAX_HEIGHT));

        Self {
            height: u16::try_from(height).expect("popup height is bounded to u16"),
            columns,
            rows,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubLayout {
    pub header: u16,
    pub separator: u16,
    pub body: u16,
}

impl HubLayout {
    pub fn new(header: u16, separator: u16, body: u16) -> Self {
        Self {
            header,
            separator,
            body: body.min(body_budget(header, separator)),
        }
    }

    pub fn with_menu(header: u16, separator: u16, body: u16, menu_separator: u16) -> Self {
        Self {
            header,
            separator,
            body: body.min(
                body_budget(header, separator)
                    .saturating_sub(menu_separator.saturating_add(1))
                    .max(1),
            ),
        }
    }

    pub fn height(self) -> u16 {
        self.header
            .saturating_add(self.separator)
            .saturating_add(self.body)
    }
}

pub fn body_budget(header: u16, separator: u16) -> u16 {
    MAX_HEIGHT
        .saturating_sub(header.saturating_add(separator))
        .max(1)
}

pub fn header_height(control: u16, vertical_padding: u16) -> u16 {
    control.saturating_add(vertical_padding.saturating_mul(2))
}

pub fn settings_body_height(rows: usize, row: u16, spacing: u16, padding: u16) -> u16 {
    let rows = u16::try_from(rows).unwrap_or(u16::MAX);
    rows.saturating_mul(row)
        .saturating_add(rows.saturating_sub(1).saturating_mul(spacing))
        .saturating_add(padding.saturating_mul(2))
        .clamp(1, MAX_HEIGHT)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Wing {
    Start,
    Center,
    #[default]
    End,
}

pub fn hub_leads(wing: Wing) -> bool {
    matches!(wing, Wing::Start)
}

pub fn hub_slot(wing: Wing, slots: usize) -> usize {
    if hub_leads(wing) {
        0
    } else {
        slots.saturating_sub(1)
    }
}

pub fn pinned_slot(wing: Wing, index: usize) -> usize {
    if hub_leads(wing) { index + 1 } else { index }
}

pub fn panel_slot_rect(slot: usize, button: (u16, u16), horizontal: bool) -> Rectangle<i32> {
    let (width, height) = (i32::from(button.0), i32::from(button.1));
    let offset = i32::try_from(slot)
        .unwrap_or(i32::MAX)
        .saturating_mul(if horizontal { width } else { height });

    Rectangle {
        x: if horizontal { offset } else { 0 },
        y: if horizontal { 0 } else { offset },
        width,
        height,
    }
}

pub fn item_grid<'a, Message: 'static + Clone>(
    items: impl IntoIterator<Item = Element<'a, Message>>,
    spacing: u16,
    height: u16,
) -> Element<'a, Message> {
    let grid = flex_row(items.into_iter().collect())
        .spacing(spacing)
        .align_items(Alignment::Center)
        .justify_items(Alignment::Start);

    container(grid)
        .width(Length::Fill)
        .height(Length::Fixed(f32::from(height)))
        .padding([HEADER_PADDING, CONTENT_PADDING])
        .align_x(Alignment::Start)
        .align_y(Alignment::Center)
        .into()
}

fn themed_container<'a, Message: 'static>(
    content: impl Into<Element<'a, Message>>,
    width: Length,
    height: Length,
) -> Element<'a, Message> {
    container(content)
        .width(width)
        .height(height)
        .style(|theme| {
            let cosmic = theme.cosmic();
            let background = cosmic.background(theme.transparent);
            cosmic::iced::widget::container::Style {
                text_color: Some(background.on.into()),
                background: Some(Color::from(background.base).into()),
                border: Border {
                    radius: cosmic.corner_radii.radius_m.into(),
                    width: 1.0,
                    color: background.divider.into(),
                },
                shadow: Shadow::default(),
                icon_color: Some(background.on.into()),
                snap: true,
            }
        })
        .into()
}

fn fixed_body<'a, Message: 'static>(
    content: impl Into<Element<'a, Message>>,
    height: u16,
    vertical_padding: u16,
) -> Element<'a, Message> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fixed(f32::from(height)))
        .padding([vertical_padding, CONTENT_PADDING])
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

pub fn selected_item<'a, Message: 'static>(
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(content)
        .style(|theme| {
            let cosmic = theme.cosmic();
            cosmic::iced::widget::container::Style {
                text_color: None,
                background: Some(Color::from(cosmic.text_button.hover).into()),
                border: Border {
                    radius: cosmic.corner_radii.radius_xl.into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                shadow: Shadow::default(),
                icon_color: None,
                snap: true,
            }
        })
        .into()
}

fn separator<'a, Message: 'static>() -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    container(divider::horizontal::default())
        .padding([0, spacing.space_s])
        .into()
}

fn menu_separator<'a, Message: 'static>() -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    container(divider::horizontal::default())
        .padding([0, spacing.space_s])
        .into()
}

pub fn separator_height() -> u16 {
    1
}

pub fn menu_separator_height() -> u16 {
    1
}

pub fn size_limits() -> Limits {
    Limits::NONE
        .min_width(1.0)
        .max_width(f32::from(SURFACE_WIDTH))
        .min_height(1.0)
        .max_height(f32::from(MAX_HEIGHT))
}

pub fn header<Message: 'static + Clone>(
    title: String,
    action: Element<'_, Message>,
    height: u16,
    vertical_padding: u16,
    dismiss_menu: Option<Message>,
) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let title =
        container(text::heading(title).ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1))))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::Center);
    let title: Element<'_, Message> = match dismiss_menu {
        Some(message) => mouse_area(title).on_press(message).into(),
        None => title.into(),
    };

    container(
        cosmic::widget::row::with_children(vec![title, action])
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs),
    )
    .width(Length::Fill)
    .height(Length::Fixed(f32::from(height)))
    .padding([vertical_padding, CONTENT_PADDING])
    .align_y(Alignment::Center)
    .into()
}

pub fn panel_item_capacity(
    suggested_bounds: Option<Size>,
    button: (u16, u16),
    horizontal: bool,
) -> usize {
    let Some(bounds) = suggested_bounds else {
        return usize::MAX;
    };
    let available = if horizontal {
        bounds.width
    } else {
        bounds.height
    };
    if available <= 0.0 {
        return usize::MAX;
    }

    let button_major = f32::from(if horizontal { button.0 } else { button.1 }).max(1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let slots = (available / button_major).floor() as usize;
    slots.max(1).saturating_sub(1)
}

pub fn panel_size_limits(suggested_bounds: Option<Size>, horizontal: bool) -> Limits {
    let Some(bounds) = suggested_bounds else {
        return Limits::NONE;
    };
    let mut limits = Limits::NONE;

    if horizontal {
        if bounds.width > 0.0 {
            limits = limits.max_width(bounds.width);
        }
        if bounds.height > 0.0 {
            limits = limits.height(bounds.height);
        }
    } else {
        if bounds.width > 0.0 {
            limits = limits.width(bounds.width);
        }
        if bounds.height > 0.0 {
            limits = limits.max_height(bounds.height);
        }
    }

    limits
}

pub fn panel_surface<'a, Message: 'static>(
    content: impl Into<Element<'a, Message>>,
    suggested_bounds: Option<Size>,
    horizontal: bool,
) -> Element<'a, Message> {
    autosize::autosize(content, PANEL_AUTOSIZE_ID.clone())
        .limits(panel_size_limits(suggested_bounds, horizontal))
        .into()
}

pub fn notice_height(row: u16) -> u16 {
    row.saturating_add(NOTICE_PADDING.saturating_mul(2))
        .min(MAX_HEIGHT)
}

pub fn notice<'a, Message: 'static>(message: String, height: u16) -> Element<'a, Message> {
    let message = text::body(message)
        .center()
        .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(2)));

    fixed_body(message, height, NOTICE_PADDING)
}

pub fn settings_list<'a, Message: 'static>(
    rows: impl IntoIterator<Item = Element<'a, Message>>,
    height: u16,
    spacing: u16,
    padding: u16,
) -> Element<'a, Message> {
    let list = cosmic::widget::column::with_children(rows.into_iter().collect::<Vec<_>>())
        .width(Length::Fill)
        .spacing(spacing);

    container(scrollable(list))
        .width(Length::Fill)
        .height(Length::Fixed(f32::from(height)))
        .padding([padding, CONTENT_PADDING])
        .into()
}

pub fn menu_height_budget(hub_height: u16, menu_separator: u16) -> u16 {
    MAX_HEIGHT
        .saturating_sub(hub_height)
        .saturating_sub(menu_separator)
        .max(1)
}

pub fn menu_comes_first(anchor: PanelAnchor) -> bool {
    matches!(anchor, PanelAnchor::Bottom)
}

pub fn hub_surface<'a, Message: 'static>(
    header: Element<'a, Message>,
    body: Element<'a, Message>,
    menu: Option<Element<'a, Message>>,
    layout: HubLayout,
    anchor: PanelAnchor,
) -> Element<'a, Message> {
    let hub = vec![header, separator(), body];
    let children = match menu {
        Some(menu) => {
            let menu = container(scrollable(container(menu).width(Length::Fill)))
                .width(Length::Fill)
                .max_height(f32::from(menu_height_budget(
                    layout.height(),
                    menu_separator_height(),
                )))
                .into();
            if menu_comes_first(anchor) {
                let mut children = vec![menu, menu_separator()];
                children.extend(hub);
                children
            } else {
                let mut children = hub;
                children.extend([menu_separator(), menu]);
                children
            }
        }
        None => hub,
    };

    autosize::autosize(
        themed_container(
            cosmic::widget::column::with_children(children),
            Length::Fixed(f32::from(SURFACE_WIDTH)),
            Length::Shrink,
        ),
        TRAY_AUTOSIZE_ID.clone(),
    )
    .limits(size_limits())
    .into()
}

pub fn menu_surface<Message: 'static>(menu: Element<'_, Message>) -> Element<'_, Message> {
    let body = container(scrollable(container(menu).width(Length::Fill)))
        .width(Length::Fill)
        .max_height(f32::from(MAX_HEIGHT));

    autosize::autosize(
        themed_container(
            body,
            Length::Fixed(f32::from(SURFACE_WIDTH)),
            Length::Shrink,
        ),
        MENU_AUTOSIZE_ID.clone(),
    )
    .limits(size_limits())
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_item_uses_only_one_row() {
        let geometry = GridGeometry::calculate(1, 48, 40, 8, 16, 12);

        assert_eq!(geometry.height, 64);
        assert_eq!((geometry.columns, geometry.rows), (1, 1));
    }

    #[test]
    fn items_grow_until_the_width_limit() {
        let geometry = GridGeometry::calculate(6, 48, 40, 8, 16, 12);

        assert_eq!(geometry.height, 64);
        assert_eq!((geometry.columns, geometry.rows), (6, 1));
    }

    #[test]
    fn overflowing_items_wrap_onto_another_row() {
        let geometry = GridGeometry::calculate(7, 48, 40, 8, 16, 12);

        assert_eq!(geometry.height, 112);
        assert_eq!((geometry.columns, geometry.rows), (6, 2));
    }

    #[test]
    fn pinning_an_item_can_drop_the_popup_to_one_row() {
        let two_rows = GridGeometry::calculate(7, 48, 40, 8, 16, 12);
        assert_eq!(two_rows.rows, 2);

        let after_pinning_one = GridGeometry::calculate(6, 48, 40, 8, 16, 12);
        assert_eq!(after_pinning_one.rows, 1);
        assert!(after_pinning_one.height < two_rows.height);
    }

    #[test]
    fn the_empty_state_is_taller_than_the_row_it_replaces() {
        let row = GridGeometry::calculate(0, 48, 40, 8, 16, 12).height;

        assert_eq!(notice_height(row), row + NOTICE_PADDING * 2);
        assert_eq!(notice_height(MAX_HEIGHT), MAX_HEIGHT);
    }

    #[test]
    fn an_empty_grid_is_still_one_row_tall() {
        let empty = GridGeometry::calculate(0, 48, 40, 8, 16, 12);

        assert_eq!(empty.rows, 1);
        assert_eq!(
            empty.height,
            GridGeometry::calculate(1, 48, 40, 8, 16, 12).height
        );
    }

    #[test]
    fn the_header_is_as_tall_as_its_control_plus_padding() {
        assert_eq!(header_height(HEADER_CONTROL, 4), HEADER_CONTROL + 8);
        assert_eq!(header_height(HEADER_CONTROL, 0), HEADER_CONTROL);
    }

    #[test]
    fn the_menu_separator_has_no_vertical_padding() {
        assert_eq!(menu_separator_height(), 1);
    }

    #[test]
    fn panel_capacity_always_reserves_one_slot_for_the_hub() {
        assert_eq!(
            panel_item_capacity(Some(Size::new(160.0, 32.0)), (40, 32), true),
            3
        );
        assert_eq!(
            panel_item_capacity(Some(Size::new(20.0, 32.0)), (40, 32), true),
            0
        );
        assert_eq!(
            panel_item_capacity(Some(Size::new(40.0, 120.0)), (40, 30), false),
            3
        );
        assert_eq!(panel_item_capacity(None, (40, 32), true), usize::MAX);
    }

    #[test]
    fn panel_bounds_are_a_maximum_only_on_the_major_axis() {
        let horizontal = panel_size_limits(Some(Size::new(160.0, 32.0)), true);
        assert_eq!(horizontal.min(), Size::new(0.0, 32.0));
        assert_eq!(horizontal.max(), Size::new(160.0, 32.0));

        let vertical = panel_size_limits(Some(Size::new(40.0, 120.0)), false);
        assert_eq!(vertical.min(), Size::new(40.0, 0.0));
        assert_eq!(vertical.max(), Size::new(40.0, 120.0));
    }

    #[test]
    fn the_body_is_bounded_by_what_the_header_leaves_over() {
        assert_eq!(body_budget(40, 9), MAX_HEIGHT - 49);
        assert_eq!(body_budget(MAX_HEIGHT, 9), 1);
    }

    #[test]
    fn a_tall_body_never_pushes_the_popup_past_its_ceiling() {
        let layout = HubLayout::new(40, 9, MAX_HEIGHT);

        assert_eq!(layout.height(), MAX_HEIGHT);
    }

    #[test]
    fn a_visible_menu_reserves_its_separator_inside_the_height_ceiling() {
        let menu_separator = menu_separator_height();
        let layout = HubLayout::with_menu(40, 9, MAX_HEIGHT, menu_separator);

        assert_eq!(
            layout.height() + menu_separator + menu_height_budget(layout.height(), menu_separator),
            MAX_HEIGHT
        );
    }

    #[test]
    fn a_short_body_is_left_exactly_as_asked() {
        let layout = HubLayout::new(40, 9, 48);

        assert_eq!(layout.body, 48);
        assert_eq!(layout.height(), 97);
    }

    #[test]
    fn the_settings_list_is_as_tall_as_the_rows_it_holds() {
        assert_eq!(
            settings_body_height(3, SETTINGS_ROW, 8, 12),
            3 * SETTINGS_ROW + 2 * 8 + 24
        );
        assert_eq!(settings_body_height(0, SETTINGS_ROW, 8, 0), 1);
    }

    #[test]
    fn the_menu_stays_on_the_side_away_from_the_panel() {
        assert!(menu_comes_first(PanelAnchor::Bottom));
        assert!(!menu_comes_first(PanelAnchor::Top));
        assert!(!menu_comes_first(PanelAnchor::Left));
        assert!(!menu_comes_first(PanelAnchor::Right));
    }

    #[test]
    fn the_hub_sits_on_the_edge_facing_away_from_the_screen_centre() {
        assert!(hub_leads(Wing::Start));
        assert!(!hub_leads(Wing::End));
        assert!(!hub_leads(Wing::Center));
    }

    #[test]
    fn the_hub_slot_is_constant_wherever_it_leads() {
        for slots in 1..6 {
            assert_eq!(hub_slot(Wing::Start, slots), 0);
        }
        assert_eq!(hub_slot(Wing::End, 4), 3);
        assert_eq!(hub_slot(Wing::Center, 4), 3);
        assert_eq!(hub_slot(Wing::End, 0), 0);
    }

    #[test]
    fn pinned_items_start_after_a_leading_hub() {
        assert_eq!(pinned_slot(Wing::Start, 0), 1);
        assert_eq!(pinned_slot(Wing::Start, 2), 3);
        assert_eq!(pinned_slot(Wing::End, 0), 0);
        assert_eq!(pinned_slot(Wing::End, 2), 2);
    }

    #[test]
    fn a_pinned_item_is_anchored_to_its_own_slot() {
        assert_eq!(
            panel_slot_rect(2, (32, 28), true),
            Rectangle {
                x: 64,
                y: 0,
                width: 32,
                height: 28
            }
        );
        assert_eq!(
            panel_slot_rect(2, (32, 28), false),
            Rectangle {
                x: 0,
                y: 56,
                width: 32,
                height: 28
            }
        );
        assert_eq!(
            panel_slot_rect(0, (32, 28), true),
            Rectangle {
                x: 0,
                y: 0,
                width: 32,
                height: 28
            }
        );
    }

    #[test]
    fn the_menu_budget_includes_the_separator() {
        let layout = HubLayout::new(40, 9, 48);
        let menu_separator = menu_separator_height();
        assert_eq!(
            menu_height_budget(layout.height(), menu_separator),
            MAX_HEIGHT - layout.height() - menu_separator
        );
    }
}
