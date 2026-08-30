use crate::core::model::TrayItem;

pub const MATCH_THRESHOLD: u32 = 40;

const MIN_HINT: usize = 3;

const NOISE: [&str; 19] = [
    "app",
    "application",
    "client",
    "com",
    "desktop",
    "electron",
    "github",
    "gtk",
    "icon",
    "indicator",
    "io",
    "net",
    "notification",
    "org",
    "panel",
    "status",
    "symbolic",
    "tray",
    "www",
];

pub fn hints(item: &TrayItem) -> Vec<String> {
    let tooltip = item
        .tooltip
        .as_ref()
        .map_or("", |tooltip| tooltip.title.as_str());

    let sources = [
        item.id.as_str(),
        item.title.as_str(),
        tooltip,
        item.key.id.as_str(),
        item.icon.icon_name.as_str(),
    ];

    let mut out = Vec::new();
    for source in sources {
        for segment in segments(source) {
            push(&mut out, segment);
        }
    }
    out
}

pub fn score(hints: &[String], app_id: &str, title: &str) -> u32 {
    let app = normalize(app_id);
    let app_segments = segments(app_id);
    let window_segments = segments(title);

    hints
        .iter()
        .map(|hint| {
            if !app.is_empty() && app == *hint {
                return 100;
            }
            if app_segments.contains(hint) {
                return 80;
            }
            if (hint.len() >= 4 && app.contains(hint.as_str()))
                || (app.len() >= 4 && hint.contains(app.as_str()))
            {
                return 60;
            }
            if window_segments.contains(hint) {
                return 40;
            }
            0
        })
        .max()
        .unwrap_or(0)
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn segments(value: &str) -> Vec<String> {
    value
        .split(|c: char| !c.is_alphanumeric())
        .map(normalize)
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn push(out: &mut Vec<String>, hint: String) {
    if hint.len() < MIN_HINT || NOISE.contains(&hint.as_str()) || out.contains(&hint) {
        return;
    }
    out.push(hint);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{IconSource, ItemKey, ToolTip};
    use std::sync::Arc;

    fn tooltip(title: &str) -> ToolTip {
        ToolTip {
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
            title: title.to_owned(),
            description: String::new(),
        }
    }

    fn icon(name: &str) -> Arc<IconSource> {
        Arc::new(IconSource {
            icon_name: name.to_owned(),
            ..IconSource::default()
        })
    }

    #[test]
    fn a_chromium_item_is_reduced_to_the_application_name() {
        let mut item = crate::core::testing::item("Mattermost_status_icon_1", 1);
        item.title = String::new();
        item.tooltip = Some(tooltip("Mattermost"));

        let hints = hints(&item);

        assert_eq!(hints, vec!["mattermost".to_owned()]);
    }

    #[test]
    fn generic_words_never_become_hints() {
        let mut item = crate::core::testing::item("tray", 1);
        item.title = "Status".to_owned();
        item.icon = icon("org.example.App.tray-symbolic");

        let hints = hints(&item);

        assert_eq!(hints, vec!["example".to_owned()]);
    }

    #[test]
    fn a_reverse_dns_app_id_matches_on_its_middle_segment() {
        let hints = vec!["mattermost".to_owned()];

        assert!(score(&hints, "com.mattermost.Desktop", "Mattermost") >= MATCH_THRESHOLD);
    }

    #[test]
    fn a_bare_app_id_matches_exactly() {
        let hints = vec!["slack".to_owned()];

        assert_eq!(score(&hints, "Slack", ""), 100);
    }

    #[test]
    fn a_window_title_carries_the_match_when_the_app_id_does_not() {
        let hints = vec!["discord".to_owned()];

        assert!(score(&hints, "electron", "#general | Discord") >= MATCH_THRESHOLD);
    }

    #[test]
    fn an_unrelated_window_stays_below_the_threshold() {
        let hints = vec!["slack".to_owned(), "mattermost".to_owned()];

        assert!(score(&hints, "org.gnome.TextEditor", "Untitled Document") < MATCH_THRESHOLD);
        assert!(score(&hints, "firefox", "Mozilla Firefox") < MATCH_THRESHOLD);
    }

    #[test]
    fn a_stronger_match_outranks_a_weaker_one() {
        let hints = vec!["slack".to_owned()];

        let by_app_id = score(&hints, "Slack", "");
        let by_title = score(&hints, "electron", "Slack");

        assert!(by_app_id > by_title);
    }

    #[test]
    fn hints_are_deduplicated() {
        let mut item = crate::core::testing::item("slack", 1);
        item.title = "Slack".to_owned();
        item.tooltip = Some(tooltip("slack"));

        assert_eq!(hints(&item), vec!["slack".to_owned()]);
    }

    #[test]
    fn an_item_with_nothing_to_go_on_yields_no_hints() {
        let mut item = crate::core::testing::item("", 1);
        item.title = String::new();
        item.key = ItemKey::new("", 0);

        assert!(hints(&item).is_empty());
    }
}
