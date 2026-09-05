use cosmic::Element;
use cosmic::applet::menu_button;
use cosmic::iced::border::Radius;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::button::{Catalog as _, Style};
use cosmic::widget::{container, divider, icon, image, space, text};

use crate::applet::message::Message;
use crate::core::menu::{EntryKind, MenuEntry, MenuIcon, MenuModel, Toggle};

const GLYPH: u16 = 16;

const INDENT_STEPS: u16 = 3;

const SURFACE_BORDER: f32 = 1.0;

const UNAVAILABLE_ALPHA: f32 = 0.5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    pub const NONE: Self = Self {
        top: false,
        bottom: false,
    };
    pub const TOP: Self = Self {
        top: true,
        bottom: false,
    };
    pub const BOTTOM: Self = Self {
        top: false,
        bottom: true,
    };
    pub const BOTH: Self = Self {
        top: true,
        bottom: true,
    };
}

struct Row<'a> {
    entry: &'a MenuEntry,
    depth: u16,
}

pub fn view<'a>(model: &'a MenuModel, expanded: &'a [i32], edges: Edges) -> Element<'a, Message> {
    let mut flat = Vec::with_capacity(model.entries.len());
    flatten(&model.entries, expanded, 0, &mut flat);

    let first = flat
        .iter()
        .position(|row| row.entry.kind != EntryKind::Separator);
    let last = flat
        .iter()
        .rposition(|row| row.entry.kind != EntryKind::Separator);

    let rows: Vec<Element<'a, Message>> = flat
        .iter()
        .enumerate()
        .map(|(index, row)| {
            if row.entry.kind == EntryKind::Separator {
                return separator();
            }
            row_for(
                row.entry,
                expanded,
                row.depth,
                Edges {
                    top: edges.top && first == Some(index),
                    bottom: edges.bottom && last == Some(index),
                },
            )
        })
        .collect();

    cosmic::widget::column::with_children(rows)
        .width(Length::Fill)
        .into()
}

fn flatten<'a>(entries: &'a [MenuEntry], expanded: &'a [i32], depth: u16, out: &mut Vec<Row<'a>>) {
    for entry in entries {
        out.push(Row { entry, depth });
        if entry.kind == EntryKind::Submenu && expanded.contains(&entry.id) {
            flatten(&entry.children, expanded, depth + 1, out);
        }
    }
}

fn separator<'a>() -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    container(divider::horizontal::default())
        .padding([0, spacing.space_s])
        .into()
}

fn row_for<'a>(
    entry: &'a MenuEntry,
    expanded: &'a [i32],
    depth: u16,
    edges: Edges,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let is_submenu = entry.kind == EntryKind::Submenu;
    let mut children: Vec<Element<'a, Message>> = Vec::new();

    if depth > 0 {
        children.push(
            space::horizontal()
                .width(Length::Fixed(f32::from(
                    depth * INDENT_STEPS * spacing.space_xxs,
                )))
                .into(),
        );
    }

    match &entry.icon {
        MenuIcon::Name(name) => children.push(
            icon::from_name(name.clone())
                .size(GLYPH)
                .symbolic(true)
                .icon()
                .into(),
        ),
        MenuIcon::Data(bytes) => children.push(
            image(image::Handle::from_bytes(bytes.as_ref().clone()))
                .width(Length::Fixed(f32::from(GLYPH)))
                .height(Length::Fixed(f32::from(GLYPH)))
                .content_fit(cosmic::iced::ContentFit::Contain)
                .into(),
        ),
        MenuIcon::None => {}
    }

    children.push(text::body(entry.label.as_str()).width(Length::Fill).into());

    if let Some(toggle) = entry.toggle {
        let mark = match toggle {
            Toggle::Checkmark(true) | Toggle::Radio(true) => Some("emblem-ok-symbolic"),
            Toggle::Checkmark(false) | Toggle::Radio(false) => None,
            Toggle::Indeterminate => Some("minus-symbolic"),
        };
        if let Some(mark) = mark {
            children.push(
                icon::from_name(mark)
                    .size(GLYPH)
                    .symbolic(true)
                    .icon()
                    .into(),
            );
        }
    }

    if is_submenu {
        let arrow = if expanded.contains(&entry.id) {
            "go-down-symbolic"
        } else {
            "go-next-symbolic"
        };
        children.push(
            icon::from_name(arrow)
                .size(GLYPH)
                .symbolic(true)
                .icon()
                .into(),
        );
    }

    let row = cosmic::widget::row::with_children(children)
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center);

    let mut button = menu_button(row).class(menu_row(edges));
    if entry.enabled {
        button = button.on_press(Message::MenuEntry {
            id: entry.id,
            submenu: is_submenu,
        });
    }

    button.into()
}

fn menu_row(edges: Edges) -> cosmic::theme::Button {
    const CLASS: cosmic::theme::Button = cosmic::theme::Button::AppletMenu;

    cosmic::theme::Button::Custom {
        active: Box::new(move |focused, theme| {
            follow_surface(theme.active(focused, false, &CLASS), theme, edges)
        }),
        disabled: Box::new(move |theme| {
            dim(follow_surface(theme.disabled(&CLASS), theme, edges), theme)
        }),
        hovered: Box::new(move |focused, theme| {
            follow_surface(theme.hovered(focused, false, &CLASS), theme, edges)
        }),
        pressed: Box::new(move |focused, theme| {
            follow_surface(theme.pressed(focused, false, &CLASS), theme, edges)
        }),
    }
}

fn follow_surface(mut style: Style, theme: &cosmic::Theme, edges: Edges) -> Style {
    style.border_radius = inner_radius(theme.cosmic().corner_radii.radius_m, edges);
    style
}

fn dim(mut style: Style, theme: &cosmic::Theme) -> Style {
    let mut unavailable = theme.cosmic().background(theme.transparent).on;
    unavailable.alpha *= UNAVAILABLE_ALPHA;
    style.text_color = Some(unavailable.into());
    style.icon_color = Some(unavailable.into());
    style
}

fn inner_radius(surface: [f32; 4], edges: Edges) -> Radius {
    let inset = |corner: f32| (corner - SURFACE_BORDER).max(0.0);

    Radius {
        top_left: if edges.top { inset(surface[0]) } else { 0.0 },
        top_right: if edges.top { inset(surface[1]) } else { 0.0 },
        bottom_right: if edges.bottom { inset(surface[2]) } else { 0.0 },
        bottom_left: if edges.bottom { inset(surface[3]) } else { 0.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::menu::MenuEntry;

    fn entry(id: i32, kind: EntryKind) -> MenuEntry {
        MenuEntry {
            id,
            kind,
            label: format!("entry {id}"),
            enabled: true,
            toggle: None,
            icon: MenuIcon::None,
            children: Vec::new(),
        }
    }

    fn flat(entries: &[MenuEntry], expanded: &[i32]) -> Vec<i32> {
        let mut rows = Vec::new();
        flatten(entries, expanded, 0, &mut rows);
        rows.iter().map(|row| row.entry.id).collect()
    }

    #[test]
    fn a_collapsed_submenu_keeps_its_children_out_of_the_list() {
        let mut parent = entry(1, EntryKind::Submenu);
        parent.children = vec![entry(2, EntryKind::Standard)];
        let entries = vec![parent, entry(3, EntryKind::Standard)];

        assert_eq!(flat(&entries, &[]), vec![1, 3]);
        assert_eq!(flat(&entries, &[1]), vec![1, 2, 3]);
    }

    #[test]
    fn nested_submenus_are_indented_one_step_per_level() {
        let mut inner = entry(3, EntryKind::Submenu);
        inner.children = vec![entry(4, EntryKind::Standard)];
        let mut outer = entry(2, EntryKind::Submenu);
        outer.children = vec![inner];
        let entries = vec![outer];

        let mut rows = Vec::new();
        flatten(&entries, &[2, 3], 0, &mut rows);

        assert_eq!(
            rows.iter().map(|row| row.depth).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn only_the_corners_that_touch_the_surface_are_rounded() {
        let surface = [8.0, 8.0, 8.0, 8.0];

        assert_eq!(inner_radius(surface, Edges::NONE), Radius::from(0.0));
        assert_eq!(
            inner_radius(surface, Edges::BOTTOM),
            Radius {
                top_left: 0.0,
                top_right: 0.0,
                bottom_right: 7.0,
                bottom_left: 7.0,
            }
        );
        assert_eq!(
            inner_radius(surface, Edges::TOP),
            Radius {
                top_left: 7.0,
                top_right: 7.0,
                bottom_right: 0.0,
                bottom_left: 0.0,
            }
        );
    }

    #[test]
    fn a_square_theme_stays_square() {
        assert_eq!(
            inner_radius([0.0, 0.0, 0.0, 0.0], Edges::BOTH),
            Radius::from(0.0)
        );
    }
}
