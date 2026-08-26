use std::sync::LazyLock;

use cosmic::Element;
use cosmic::applet::cosmic_panel_config::PanelAnchor;
use cosmic::iced::{Alignment, Border, Color, Length, Limits, Shadow};
use cosmic::widget::{autosize, container, divider, flex_row, scrollable};

pub const SURFACE_WIDTH: u16 = 360;
const MAX_HEIGHT: u16 = 1080;

static TRAY_AUTOSIZE_ID: LazyLock<cosmic::iced::id::Id> =
    LazyLock::new(|| cosmic::iced::id::Id::new("cosmic-status-hub-tray-popup"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridGeometry {
    pub width: u16,
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
        padding: u32,
    ) -> Self {
        let usable_width = u32::from(SURFACE_WIDTH).saturating_sub(padding.saturating_mul(2));
        let columns_at_limit = usable_width
            .saturating_add(spacing)
            .checked_div(item_width.saturating_add(spacing).max(1))
            .unwrap_or(1)
            .max(1);
        let columns_at_limit = usize::try_from(columns_at_limit).unwrap_or(usize::MAX);
        let columns = item_count.clamp(1, columns_at_limit);
        let rows = item_count.max(1).div_ceil(columns);
        let columns_u32 = u32::try_from(columns).unwrap_or(u32::MAX);
        let rows_u32 = u32::try_from(rows).unwrap_or(u32::MAX);
        let content_width = columns_u32
            .saturating_mul(item_width)
            .saturating_add(columns_u32.saturating_sub(1).saturating_mul(spacing));
        let natural_width = content_width.saturating_add(padding.saturating_mul(2));
        let width = if item_count > columns_at_limit {
            u32::from(SURFACE_WIDTH)
        } else {
            natural_width.min(u32::from(SURFACE_WIDTH))
        }
        .max(1);
        let content_height = rows_u32
            .saturating_mul(item_height)
            .saturating_add(rows_u32.saturating_sub(1).saturating_mul(spacing));
        let height = content_height
            .saturating_add(padding.saturating_mul(2))
            .clamp(1, u32::from(MAX_HEIGHT));

        Self {
            width: u16::try_from(width).expect("popup width is bounded to u16"),
            height: u16::try_from(height).expect("popup height is bounded to u16"),
            columns,
            rows,
        }
    }
}

pub fn item_grid<'a, Message: 'static + Clone>(
    items: impl IntoIterator<Item = Element<'a, Message>>,
    spacing: u16,
) -> Element<'a, Message> {
    flex_row(items.into_iter().collect())
        .spacing(spacing)
        .align_items(Alignment::Center)
        .justify_items(Alignment::Center)
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

fn icon_row<'a, Message: 'static>(
    content: impl Into<Element<'a, Message>>,
    geometry: GridGeometry,
    padding: u16,
) -> Element<'a, Message> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fixed(f32::from(geometry.height)))
        .padding(padding)
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

fn menu_body<'a, Message: 'static>(
    content: impl Into<Element<'a, Message>>,
    max_height: u16,
) -> Element<'a, Message> {
    container(scrollable(container(content).width(Length::Fill)))
        .width(Length::Fill)
        .max_height(f32::from(max_height))
        .into()
}

fn separator<'a, Message: 'static>() -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    container(divider::horizontal::default())
        .padding([spacing.space_xxxs, spacing.space_s])
        .into()
}

fn separator_height() -> u16 {
    cosmic::theme::spacing()
        .space_xxxs
        .saturating_mul(2)
        .saturating_add(1)
}

fn menu_comes_first(anchor: PanelAnchor) -> bool {
    matches!(anchor, PanelAnchor::Bottom)
}

fn menu_height_budget(icon_row_height: u16, separator_height: u16) -> u16 {
    MAX_HEIGHT
        .saturating_sub(icon_row_height.saturating_add(separator_height))
        .max(1)
}

pub fn panel_icon(anchor: PanelAnchor, open: bool) -> &'static str {
    match (anchor, open) {
        (PanelAnchor::Top, false) | (PanelAnchor::Bottom, true) => "go-down-symbolic",
        (PanelAnchor::Top, true) | (PanelAnchor::Bottom, false) => "go-up-symbolic",
        (PanelAnchor::Left, false) | (PanelAnchor::Right, true) => "go-next-symbolic",
        (PanelAnchor::Left, true) | (PanelAnchor::Right, false) => "go-previous-symbolic",
    }
}

pub fn size_limits() -> Limits {
    Limits::NONE
        .min_width(1.0)
        .max_width(f32::from(SURFACE_WIDTH))
        .min_height(1.0)
        .max_height(f32::from(MAX_HEIGHT))
}

pub fn surface_container<'a, Message: 'static>(
    grid: impl Into<Element<'a, Message>>,
    menu: Option<Element<'a, Message>>,
    geometry: GridGeometry,
    padding: u16,
    anchor: PanelAnchor,
) -> Element<'a, Message> {
    let icons = icon_row(grid, geometry, padding);
    let (width, children) = match menu {
        Some(menu) => {
            let menu = menu_body(
                menu,
                menu_height_budget(geometry.height, separator_height()),
            );
            let children = if menu_comes_first(anchor) {
                vec![menu, separator(), icons]
            } else {
                vec![icons, separator(), menu]
            };
            (SURFACE_WIDTH, children)
        }
        None => (geometry.width, vec![icons]),
    };

    autosize::autosize(
        themed_container(
            cosmic::widget::column::with_children(children),
            Length::Fixed(f32::from(width)),
            Length::Shrink,
        ),
        TRAY_AUTOSIZE_ID.clone(),
    )
    .limits(size_limits())
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_item_uses_only_its_natural_width() {
        let geometry = GridGeometry::calculate(1, 48, 40, 8, 4);

        assert_eq!((geometry.width, geometry.height), (56, 48));
        assert_eq!((geometry.columns, geometry.rows), (1, 1));
    }

    #[test]
    fn items_grow_until_the_width_limit() {
        let geometry = GridGeometry::calculate(6, 48, 40, 8, 4);

        assert_eq!((geometry.width, geometry.height), (336, 48));
        assert_eq!((geometry.columns, geometry.rows), (6, 1));
    }

    #[test]
    fn overflowing_items_wrap_onto_another_row() {
        let geometry = GridGeometry::calculate(7, 48, 40, 8, 4);

        assert_eq!((geometry.width, geometry.height), (SURFACE_WIDTH, 96));
        assert_eq!((geometry.columns, geometry.rows), (6, 2));
    }

    #[test]
    fn the_chevron_points_at_where_the_popup_will_be() {
        assert_eq!(panel_icon(PanelAnchor::Top, false), "go-down-symbolic");
        assert_eq!(panel_icon(PanelAnchor::Top, true), "go-up-symbolic");
        assert_eq!(panel_icon(PanelAnchor::Bottom, false), "go-up-symbolic");
        assert_eq!(panel_icon(PanelAnchor::Bottom, true), "go-down-symbolic");
        assert_eq!(panel_icon(PanelAnchor::Left, false), "go-next-symbolic");
        assert_eq!(
            panel_icon(PanelAnchor::Right, false),
            "go-previous-symbolic"
        );
    }

    #[test]
    fn the_menu_never_pushes_the_icons_away_from_the_panel() {
        assert!(menu_comes_first(PanelAnchor::Bottom));
        assert!(!menu_comes_first(PanelAnchor::Top));
        assert!(!menu_comes_first(PanelAnchor::Left));
        assert!(!menu_comes_first(PanelAnchor::Right));
    }

    #[test]
    fn the_menu_is_bounded_by_what_the_icons_leave_over() {
        let geometry = GridGeometry::calculate(1, 48, 40, 8, 4);

        assert_eq!(menu_height_budget(geometry.height, 9), MAX_HEIGHT - 57);
        assert_eq!(menu_height_budget(MAX_HEIGHT, 9), 1);
    }
}
