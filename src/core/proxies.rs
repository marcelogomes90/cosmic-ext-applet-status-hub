use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::core::model::{Pixmap, ToolTip};

#[zbus::proxy(
    interface = "org.kde.StatusNotifierWatcher",
    default_service = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierWatcher"
)]
pub trait StatusNotifierWatcher {
    fn register_status_notifier_host(&self, service: &str) -> zbus::Result<()>;

    fn register_status_notifier_item(&self, service: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> zbus::Result<Vec<String>>;

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn protocol_version(&self) -> zbus::Result<i32>;

    #[zbus(signal)]
    fn status_notifier_item_registered(&self, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn status_notifier_item_unregistered(&self, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn status_notifier_host_registered(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn status_notifier_host_unregistered(&self) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "com.system76.CosmicStatusNotifierWatcher",
    default_service = "com.system76.CosmicStatusNotifierWatcher",
    default_path = "/CosmicStatusNotifierWatcher"
)]
pub trait CosmicStatusNotifierWatcher {
    fn register_applet(&self) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.kde.StatusNotifierItem", assume_defaults = false)]
pub trait StatusNotifierItem {
    #[zbus(property)]
    fn category(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn title(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn window_id(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_pixmap(&self) -> zbus::Result<Vec<Pixmap>>;

    #[zbus(property)]
    fn icon_theme_path(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn overlay_icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> zbus::Result<Vec<Pixmap>>;

    #[zbus(property)]
    fn attention_icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> zbus::Result<Vec<Pixmap>>;

    #[zbus(property)]
    fn attention_movie_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn tool_tip(&self) -> zbus::Result<ToolTip>;

    #[zbus(property)]
    fn menu(&self) -> zbus::Result<OwnedObjectPath>;

    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;

    fn activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    fn secondary_activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    fn scroll(&self, delta: i32, orientation: &str) -> zbus::Result<()>;

    fn provide_xdg_activation_token(&self, token: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_title(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_icon(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_attention_icon(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_overlay_icon(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_tool_tip(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_status(&self, status: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_icon_theme_path(&self, path: &str) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.DBus.Introspectable",
    assume_defaults = false
)]
pub trait Introspectable {
    fn introspect(&self) -> zbus::Result<String>;
}

#[zbus::proxy(interface = "com.canonical.dbusmenu", assume_defaults = false)]
pub trait DBusMenu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: &[&str],
    ) -> zbus::Result<(u32, RawLayout)>;

    fn get_group_properties(
        &self,
        ids: &[i32],
        property_names: &[&str],
    ) -> zbus::Result<Vec<(i32, std::collections::HashMap<String, OwnedValue>)>>;

    fn event(
        &self,
        id: i32,
        event_id: &str,
        data: &zbus::zvariant::Value<'_>,
        timestamp: u32,
    ) -> zbus::Result<()>;

    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;

    #[zbus(property)]
    fn version(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn text_direction(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_theme_path(&self) -> zbus::Result<Vec<String>>;

    #[zbus(signal)]
    fn layout_updated(&self, revision: u32, parent: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn items_properties_updated(
        &self,
        updated: Vec<(i32, std::collections::HashMap<String, OwnedValue>)>,
        removed: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    fn item_activation_requested(&self, id: i32, timestamp: u32) -> zbus::Result<()>;
}

#[derive(Clone, Debug)]
pub struct RawLayout {
    pub id: i32,
    pub properties: std::collections::HashMap<String, OwnedValue>,
    pub children: Vec<RawLayout>,
}

impl zbus::zvariant::Type for RawLayout {
    const SIGNATURE: &'static zbus::zvariant::Signature = <(
        i32,
        std::collections::HashMap<String, zbus::zvariant::Value<'static>>,
        Vec<zbus::zvariant::Value<'static>>,
    )>::SIGNATURE;
}

impl<'de> serde::Deserialize<'de> for RawLayout {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (id, properties, children) = <(
            i32,
            std::collections::HashMap<String, OwnedValue>,
            Vec<(zbus::zvariant::Signature, RawLayout)>,
        )>::deserialize(deserializer)?;

        Ok(Self {
            id,
            properties,
            children: children.into_iter().map(|(_, child)| child).collect(),
        })
    }
}
