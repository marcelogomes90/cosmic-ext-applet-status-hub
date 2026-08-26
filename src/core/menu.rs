use std::collections::HashMap;
use std::sync::Arc;

use zbus::zvariant::{OwnedValue, Value};

use crate::core::model::{Generation, ItemAddress};
use crate::core::proxies::RawLayout;

type Props = HashMap<String, OwnedValue>;

#[derive(Clone, Debug, PartialEq)]
pub struct MenuModel {
    pub owner: ItemAddress,
    pub item_generation: Generation,
    pub revision: u32,
    pub entries: Vec<MenuEntry>,
}

impl MenuModel {
    pub fn from_layout(
        owner: ItemAddress,
        item_generation: Generation,
        revision: u32,
        root: &RawLayout,
    ) -> Self {
        Self {
            owner,
            item_generation,
            revision,
            entries: parse_children(root),
        }
    }

    pub fn is_empty(&self) -> bool {
        !self
            .entries
            .iter()
            .any(|entry| !matches!(entry.kind, EntryKind::Separator))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuEntry {
    pub id: i32,
    pub kind: EntryKind,
    pub label: String,
    pub enabled: bool,
    pub toggle: Option<Toggle>,
    pub icon: MenuIcon,
    pub children: Vec<MenuEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Standard,
    Separator,
    Submenu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    Checkmark(bool),
    Radio(bool),
    Indeterminate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuIcon {
    None,
    Name(String),
    Data(Arc<Vec<u8>>),
}

fn parse_children(node: &RawLayout) -> Vec<MenuEntry> {
    node.children.iter().filter_map(parse_entry).collect()
}

fn parse_entry(node: &RawLayout) -> Option<MenuEntry> {
    let props = &node.properties;

    if !as_bool(props, "visible").unwrap_or(true) {
        return None;
    }

    let kind = if as_str(props, "type").as_deref() == Some("separator") {
        EntryKind::Separator
    } else if as_str(props, "children-display").as_deref() == Some("submenu") {
        EntryKind::Submenu
    } else {
        EntryKind::Standard
    };

    let toggle = match as_str(props, "toggle-type").as_deref() {
        Some("checkmark") => Some(match as_i32(props, "toggle-state") {
            Some(0) => Toggle::Checkmark(false),
            Some(1) => Toggle::Checkmark(true),
            _ => Toggle::Indeterminate,
        }),
        Some("radio") => Some(match as_i32(props, "toggle-state") {
            Some(0) => Toggle::Radio(false),
            Some(1) => Toggle::Radio(true),
            _ => Toggle::Indeterminate,
        }),
        _ => None,
    };

    let icon = if let Some(data) = as_bytes(props, "icon-data") {
        MenuIcon::Data(Arc::new(data))
    } else if let Some(name) = as_str(props, "icon-name").filter(|name| !name.is_empty()) {
        MenuIcon::Name(name)
    } else {
        MenuIcon::None
    };

    Some(MenuEntry {
        id: node.id,
        kind,
        label: strip_mnemonics(&as_str(props, "label").unwrap_or_default()),
        enabled: as_bool(props, "enabled").unwrap_or(true),
        toggle,
        icon,
        children: parse_children(node),
    })
}

fn strip_mnemonics(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars();

    while let Some(c) = chars.next() {
        if c != '_' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('_') => out.push('_'),
            Some(next) => out.push(next),
            None => {}
        }
    }

    out
}

fn as_str(props: &Props, key: &str) -> Option<String> {
    match &**props.get(key)? {
        Value::Str(value) => Some(value.to_string()),
        _ => None,
    }
}

fn as_bool(props: &Props, key: &str) -> Option<bool> {
    match &**props.get(key)? {
        Value::Bool(value) => Some(*value),
        _ => None,
    }
}

fn as_i32(props: &Props, key: &str) -> Option<i32> {
    match &**props.get(key)? {
        Value::I32(value) => Some(*value),
        Value::I16(value) => Some(i32::from(*value)),
        Value::U32(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn as_bytes(props: &Props, key: &str) -> Option<Vec<u8>> {
    match &**props.get(key)? {
        Value::Array(array) => {
            let bytes: Vec<u8> = array
                .iter()
                .filter_map(|value| match value {
                    Value::U8(byte) => Some(*byte),
                    _ => None,
                })
                .collect();
            (!bytes.is_empty()).then_some(bytes)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, Value<'static>)]) -> Props {
        pairs
            .iter()
            .map(|(key, value)| {
                (
                    (*key).to_owned(),
                    OwnedValue::try_from(value.clone()).unwrap(),
                )
            })
            .collect()
    }

    fn node(id: i32, properties: Props, children: Vec<RawLayout>) -> RawLayout {
        RawLayout {
            id,
            properties,
            children,
        }
    }

    fn leaf(id: i32, pairs: &[(&str, Value<'static>)]) -> RawLayout {
        node(id, props(pairs), Vec::new())
    }

    fn parse(root: &RawLayout) -> Vec<MenuEntry> {
        MenuModel::from_layout(
            crate::core::testing::address("org.example.App", ":1.1"),
            Generation(1),
            1,
            root,
        )
        .entries
    }

    #[test]
    fn parses_a_realistic_layout() {
        let root = node(
            0,
            props(&[("children-display", "submenu".into())]),
            vec![
                leaf(2, &[("label", "Minimize to Tray".into())]),
                leaf(3, &[("visible", false.into()), ("label", "Show".into())]),
                leaf(4, &[("type", "separator".into()), ("enabled", true.into())]),
                leaf(5, &[("label", "Exit".into())]),
            ],
        );

        let entries = parse(&root);
        assert_eq!(entries.len(), 3, "the invisible entry must not survive");
        assert_eq!(entries[0].label, "Minimize to Tray");
        assert_eq!(entries[0].kind, EntryKind::Standard);
        assert!(entries[0].enabled);
        assert_eq!(entries[1].kind, EntryKind::Separator);
        assert_eq!(entries[2].label, "Exit");
    }

    #[test]
    fn a_property_of_the_wrong_type_degrades_only_itself() {
        let root = node(
            0,
            Props::new(),
            vec![
                leaf(
                    1,
                    &[("label", "Good".into()), ("enabled", "yes, really".into())],
                ),
                leaf(2, &[("label", 42i32.into())]),
                leaf(3, &[("label", "Also good".into())]),
            ],
        );

        let entries = parse(&root);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].label, "Good");
        assert!(entries[0].enabled);
        assert_eq!(entries[1].label, "");
        assert_eq!(entries[2].label, "Also good");
    }

    #[test]
    fn an_unexpected_shortcut_type_does_not_cost_the_menu() {
        let root = node(
            0,
            Props::new(),
            vec![leaf(
                1,
                &[("label", "Quit".into()), ("shortcut", "Ctrl+Q".into())],
            )],
        );

        let entries = parse(&root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Quit");
    }

    #[test]
    fn submenus_are_recognised_and_recursed_into() {
        let root = node(
            0,
            Props::new(),
            vec![node(
                1,
                props(&[
                    ("label", "More".into()),
                    ("children-display", "submenu".into()),
                ]),
                vec![leaf(2, &[("label", "Nested".into())])],
            )],
        );

        let entries = parse(&root);
        assert_eq!(entries[0].kind, EntryKind::Submenu);
        assert_eq!(entries[0].children.len(), 1);
        assert_eq!(entries[0].children[0].label, "Nested");
    }

    #[test]
    fn toggles_carry_their_state() {
        let root = node(
            0,
            Props::new(),
            vec![
                leaf(
                    1,
                    &[
                        ("toggle-type", "checkmark".into()),
                        ("toggle-state", 1i32.into()),
                    ],
                ),
                leaf(
                    2,
                    &[
                        ("toggle-type", "radio".into()),
                        ("toggle-state", 0i32.into()),
                    ],
                ),
                leaf(
                    3,
                    &[
                        ("toggle-type", "checkmark".into()),
                        ("toggle-state", (-1i32).into()),
                    ],
                ),
                leaf(4, &[("label", "plain".into())]),
            ],
        );

        let entries = parse(&root);
        assert_eq!(entries[0].toggle, Some(Toggle::Checkmark(true)));
        assert_eq!(entries[1].toggle, Some(Toggle::Radio(false)));
        assert_eq!(entries[2].toggle, Some(Toggle::Indeterminate));
        assert_eq!(entries[3].toggle, None);
    }

    #[test]
    fn icon_data_wins_over_an_icon_name() {
        let root = node(
            0,
            Props::new(),
            vec![
                leaf(1, &[("icon-name", "document-open".into())]),
                leaf(
                    2,
                    &[
                        ("icon-name", "ignored".into()),
                        ("icon-data", Value::from(vec![1u8, 2, 3])),
                    ],
                ),
                leaf(3, &[("icon-name", "".into())]),
            ],
        );

        let entries = parse(&root);
        assert_eq!(entries[0].icon, MenuIcon::Name("document-open".to_owned()));
        assert_eq!(entries[1].icon, MenuIcon::Data(Arc::new(vec![1, 2, 3])));
        assert_eq!(entries[2].icon, MenuIcon::None);
    }

    #[test]
    fn mnemonic_markers_are_removed_but_literal_underscores_survive() {
        assert_eq!(strip_mnemonics("_Quit"), "Quit");
        assert_eq!(strip_mnemonics("Save _As"), "Save As");
        assert_eq!(strip_mnemonics("my__file"), "my_file");
        assert_eq!(strip_mnemonics("nothing"), "nothing");
        assert_eq!(strip_mnemonics("trailing_"), "trailing");
    }

    #[test]
    fn a_menu_of_separators_counts_as_empty() {
        let root = node(
            0,
            Props::new(),
            vec![
                leaf(1, &[("type", "separator".into())]),
                leaf(2, &[("type", "separator".into())]),
            ],
        );
        let model = MenuModel::from_layout(
            crate::core::testing::address("org.example.App", ":1.1"),
            Generation(1),
            1,
            &root,
        );
        assert!(model.is_empty());
    }
}
