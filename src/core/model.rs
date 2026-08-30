use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use zbus::names::{BusName, OwnedBusName, OwnedUniqueName};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Type, Value};

pub const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItemAddress {
    pub service: OwnedBusName,
    pub path: OwnedObjectPath,
    pub owner: OwnedUniqueName,
}

impl fmt::Display for ItemAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.service.as_str(), self.path.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseServiceError {
    pub entry: String,
    pub reason: &'static str,
}

impl fmt::Display for ParseServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid item service {:?}: {}", self.entry, self.reason)
    }
}

impl std::error::Error for ParseServiceError {}

pub fn parse_service_entry(
    entry: &str,
) -> Result<(OwnedBusName, OwnedObjectPath), ParseServiceError> {
    let err = |reason| ParseServiceError {
        entry: entry.to_owned(),
        reason,
    };

    let (name, path) = match entry.find('/') {
        Some(0) => return Err(err("entry starts with an object path and names no service")),
        Some(idx) => (&entry[..idx], &entry[idx..]),
        None => (entry, DEFAULT_ITEM_PATH),
    };

    let name = BusName::try_from(name.to_owned()).map_err(|_| err("not a valid bus name"))?;
    let path = ObjectPath::try_from(path.to_owned()).map_err(|_| err("not a valid object path"))?;

    Ok((name.into(), path.into()))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemKey {
    pub id: String,
    pub dup: u16,
}

impl ItemKey {
    pub fn new(id: impl Into<String>, dup: u16) -> Self {
        Self { id: id.into(), dup }
    }

    pub fn derive_id(id: &str, title: &str, service: &BusName<'_>) -> String {
        let id = id.trim();
        if !id.is_empty() {
            return id.to_owned();
        }
        let title = title.trim();
        if !title.is_empty() {
            return title.to_owned();
        }
        strip_instance_suffix(service.as_str())
    }
}

impl fmt::Display for ItemKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.dup == 0 {
            f.write_str(&self.id)
        } else {
            write!(f, "{}#{}", self.id, self.dup)
        }
    }
}

fn strip_instance_suffix(name: &str) -> String {
    let mut end = name.len();
    loop {
        let head = &name[..end];
        let Some(dash) = head.rfind('-') else { break };
        let tail = &head[dash + 1..];
        if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        end = dash;
    }
    if end == 0 {
        name.to_owned()
    } else {
        name[..end].to_owned()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Generation(pub u64);

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DiscoverySeq(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Category {
    #[default]
    ApplicationStatus,
    Communications,
    SystemServices,
    Hardware,
    Other(String),
}

impl From<&str> for Category {
    fn from(value: &str) -> Self {
        match value {
            "ApplicationStatus" => Self::ApplicationStatus,
            "Communications" => Self::Communications,
            "SystemServices" => Self::SystemServices,
            "Hardware" => Self::Hardware,
            other => Self::Other(other.to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ItemStatus {
    Passive,
    #[default]
    Active,
    NeedsAttention,
}

impl From<&str> for ItemStatus {
    fn from(value: &str) -> Self {
        match value {
            "Passive" => Self::Passive,
            "NeedsAttention" => Self::NeedsAttention,
            _ => Self::Active,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, Value, OwnedValue)]
pub struct Pixmap {
    pub width: i32,
    pub height: i32,
    pub bytes: Vec<u8>,
}

impl Pixmap {
    pub fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && i64::from(self.width)
                .checked_mul(i64::from(self.height))
                .and_then(|px| px.checked_mul(4))
                .is_some_and(|expected| u64::try_from(expected) == Ok(self.bytes.len() as u64))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, Value, OwnedValue)]
pub struct ToolTip {
    pub icon_name: String,
    pub icon_pixmap: Vec<Pixmap>,
    pub title: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IconSource {
    pub icon_name: String,
    pub icon_pixmap: Vec<Pixmap>,
    pub attention_icon_name: String,
    pub attention_icon_pixmap: Vec<Pixmap>,
    pub overlay_icon_name: String,
    pub overlay_icon_pixmap: Vec<Pixmap>,
    pub theme_path: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TrayItem {
    pub address: ItemAddress,
    pub key: ItemKey,
    pub generation: Generation,
    pub discovery_seq: DiscoverySeq,
    pub state: crate::core::lifecycle::LifecycleState,
    pub id: String,
    pub title: String,
    pub category: Category,
    pub status: ItemStatus,
    pub menu_path: Option<OwnedObjectPath>,
    pub tooltip: Option<ToolTip>,
    pub icon: Arc<IconSource>,
    pub takes_activation_token: bool,
}

impl TrayItem {
    pub fn label(&self) -> &str {
        if let Some(tooltip) = &self.tooltip
            && !tooltip.title.is_empty()
        {
            return &tooltip.title;
        }
        if !self.title.is_empty() {
            return &self.title;
        }
        if !self.id.is_empty() {
            return &self.id;
        }
        &self.key.id
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum WatcherState {
    #[default]
    Connecting,
    Connected,
    Unavailable(String),
}

#[derive(Clone, Debug, Default)]
pub struct TraySnapshot {
    pub items: Vec<TrayItem>,
    pub watcher: WatcherState,
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_label_prefers_a_tooltip_title() {
        let mut item = crate::core::testing::item("slack", 1);
        item.title = "Slack".to_owned();
        item.tooltip = Some(ToolTip {
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
            title: "you have a notification".to_owned(),
            description: String::new(),
        });

        assert_eq!(item.label(), "you have a notification");
    }

    #[test]
    fn a_label_falls_back_past_an_empty_title() {
        let mut item = crate::core::testing::item("slack", 1);
        item.title = String::new();
        item.id = "slack-client".to_owned();

        assert_eq!(item.label(), "slack-client");
    }

    use super::*;

    #[test]
    fn parses_concatenated_unique_name_and_path() {
        let (name, path) = parse_service_entry(":1.42/StatusNotifierItem").unwrap();
        assert_eq!(name.as_str(), ":1.42");
        assert_eq!(path.as_str(), "/StatusNotifierItem");
    }

    #[test]
    fn parses_bare_well_known_name_with_default_path() {
        let (name, path) = parse_service_entry("org.kde.StatusNotifierItem-1234-1").unwrap();
        assert_eq!(name.as_str(), "org.kde.StatusNotifierItem-1234-1");
        assert_eq!(path.as_str(), DEFAULT_ITEM_PATH);
    }

    #[test]
    fn parses_non_default_object_path() {
        let (name, path) =
            parse_service_entry("org.example.App/org/ayatana/NotificationItem/x").unwrap();
        assert_eq!(name.as_str(), "org.example.App");
        assert_eq!(path.as_str(), "/org/ayatana/NotificationItem/x");
    }

    #[test]
    fn rejects_entries_without_a_service() {
        assert!(parse_service_entry("/StatusNotifierItem").is_err());
        assert!(parse_service_entry("").is_err());
        assert!(parse_service_entry("not a bus name").is_err());
    }

    #[test]
    fn stable_id_prefers_id_then_title_then_stripped_name() {
        let service = BusName::try_from("org.kde.StatusNotifierItem-1234-1").unwrap();
        assert_eq!(ItemKey::derive_id("steam", "Steam", &service), "steam");
        assert_eq!(ItemKey::derive_id("  ", "Steam", &service), "Steam");
        assert_eq!(
            ItemKey::derive_id("", "", &service),
            "org.kde.StatusNotifierItem"
        );
    }

    #[test]
    fn stripping_leaves_names_without_numeric_suffix_alone() {
        assert_eq!(strip_instance_suffix("org.example.App"), "org.example.App");
        assert_eq!(
            strip_instance_suffix("org.example.App-beta"),
            "org.example.App-beta"
        );
        assert_eq!(strip_instance_suffix("1234"), "1234");
    }

    #[test]
    fn pixmap_validity_requires_matching_byte_count() {
        assert!(
            Pixmap {
                width: 2,
                height: 2,
                bytes: vec![0; 16]
            }
            .is_valid()
        );
        assert!(
            !Pixmap {
                width: 2,
                height: 2,
                bytes: vec![0; 15]
            }
            .is_valid()
        );
        assert!(
            !Pixmap {
                width: 0,
                height: 2,
                bytes: Vec::new()
            }
            .is_valid()
        );
        assert!(
            !Pixmap {
                width: -1,
                height: 2,
                bytes: vec![0; 8]
            }
            .is_valid()
        );
    }

    #[test]
    fn unknown_status_is_treated_as_active() {
        assert_eq!(ItemStatus::from("Passive"), ItemStatus::Passive);
        assert_eq!(
            ItemStatus::from("NeedsAttention"),
            ItemStatus::NeedsAttention
        );
        assert_eq!(ItemStatus::from("bogus"), ItemStatus::Active);
    }
}
