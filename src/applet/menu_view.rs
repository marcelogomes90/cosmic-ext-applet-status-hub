use cosmic::Element;
use cosmic::applet::menu_button;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{container, divider, icon, image, space, text};

use crate::applet::message::Message;
use crate::core::menu::{EntryKind, MenuEntry, MenuIcon, MenuModel, Toggle};

const GLYPH: u16 = 14;

const INDENT_STEPS: u16 = 3;

pub fn view<'a>(model: &'a MenuModel, expanded: &'a [i32]) -> Element<'a, Message> {
    let mut rows = Vec::with_capacity(model.entries.len());
    for entry in &model.entries {
        push_entry(&mut rows, entry, expanded, 0);
    }

    cosmic::widget::column::with_children(rows)
        .width(Length::Fill)
        .into()
}

fn push_entry<'a>(
    rows: &mut Vec<Element<'a, Message>>,
    entry: &'a MenuEntry,
    expanded: &'a [i32],
    depth: u16,
) {
    let spacing = cosmic::theme::spacing();

    match entry.kind {
        EntryKind::Separator => rows.push(
            container(divider::horizontal::default())
                .padding([0, spacing.space_s])
                .into(),
        ),

        EntryKind::Standard => rows.push(row_for(entry, expanded, depth, false)),

        EntryKind::Submenu => {
            rows.push(row_for(entry, expanded, depth, true));
            if expanded.contains(&entry.id) {
                for child in &entry.children {
                    push_entry(rows, child, expanded, depth + 1);
                }
            }
        }
    }
}

fn row_for<'a>(
    entry: &'a MenuEntry,
    expanded: &'a [i32],
    depth: u16,
    is_submenu: bool,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
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

    let mut button = menu_button(row);
    if entry.enabled {
        button = button.on_press(Message::MenuEntry {
            id: entry.id,
            submenu: is_submenu,
        });
    }

    button.into()
}
