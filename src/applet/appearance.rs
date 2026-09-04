use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet, CosmicConfigEntry, Error};

use crate::applet::pins::CONFIG_VERSION;

pub const COLOUR_ICONS_KEY: &str = "colour-icons";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appearance {
    colour_icons: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { colour_icons: true }
    }
}

impl Appearance {
    pub fn colour_icons(self) -> bool {
        self.colour_icons
    }

    pub fn set_colour_icons(&mut self, colour_icons: bool) {
        self.colour_icons = colour_icons;
    }
}

impl CosmicConfigEntry for Appearance {
    const VERSION: u64 = CONFIG_VERSION;

    fn write_entry(&self, config: &Config) -> Result<(), Error> {
        config.set(COLOUR_ICONS_KEY, self.colour_icons)
    }

    fn get_entry(config: &Config) -> Result<Self, (Vec<Error>, Self)> {
        match config.get::<bool>(COLOUR_ICONS_KEY) {
            Ok(colour_icons) => Ok(Self { colour_icons }),
            Err(error) => Err((vec![error], Self::default())),
        }
    }

    fn update_keys<T: AsRef<str>>(
        &mut self,
        config: &Config,
        changed_keys: &[T],
    ) -> (Vec<Error>, Vec<&'static str>) {
        let mut errors = Vec::new();
        let mut updated = Vec::new();

        for key in changed_keys {
            if key.as_ref() != COLOUR_ICONS_KEY {
                continue;
            }
            match config.get::<bool>(COLOUR_ICONS_KEY) {
                Ok(colour_icons) if self.colour_icons != colour_icons => {
                    self.colour_icons = colour_icons;
                    updated.push(COLOUR_ICONS_KEY);
                }
                Ok(_) => {}
                Err(error) => errors.push(error),
            }
        }

        (errors, updated)
    }
}

pub struct AppearanceStore {
    config: Option<Config>,
}

impl AppearanceStore {
    pub fn open(app_id: &str) -> Self {
        match Config::new(app_id, CONFIG_VERSION) {
            Ok(config) => Self {
                config: Some(config),
            },
            Err(error) => {
                tracing::warn!(%error, "appearance will not persist, the config is unavailable");
                Self { config: None }
            }
        }
    }

    pub fn load(&self) -> Appearance {
        let Some(config) = self.config.as_ref() else {
            return Appearance::default();
        };

        match config.get::<bool>(COLOUR_ICONS_KEY) {
            Ok(colour_icons) => Appearance { colour_icons },
            Err(error) => {
                tracing::debug!(%error, "no appearance stored yet");
                Appearance::default()
            }
        }
    }

    pub fn save(&self, appearance: Appearance) {
        let Some(config) = self.config.as_ref() else {
            return;
        };

        if let Err(error) = config.set(COLOUR_ICONS_KEY, appearance.colour_icons) {
            tracing::warn!(%error, "could not store the appearance");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(name: &str) -> (AppearanceStore, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "cosmic-status-hub-appearance-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        let config = Config::with_custom_path(name, CONFIG_VERSION, path.clone())
            .expect("a config under a temporary path");
        (
            AppearanceStore {
                config: Some(config),
            },
            path,
        )
    }

    #[test]
    fn icons_follow_the_theme_until_the_user_chooses_otherwise() {
        assert!(Appearance::default().colour_icons());
    }

    #[test]
    fn the_original_icon_preference_survives_a_round_trip() {
        let (store, path) = test_store("appearance-round-trip");
        let mut appearance = Appearance::default();
        appearance.set_colour_icons(false);

        store.save(appearance);

        assert_eq!(store.load(), appearance);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn a_missing_preference_keeps_icons_adapted_to_the_theme() {
        let (store, path) = test_store("appearance-default");

        assert!(store.load().colour_icons());
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn a_changed_preference_is_reported_once() {
        let (store, path) = test_store("appearance-change");
        let config = store.config.as_ref().unwrap();
        config.set(COLOUR_ICONS_KEY, false).unwrap();
        let mut appearance = Appearance::default();

        let (errors, changed) = appearance.update_keys(config, &[COLOUR_ICONS_KEY]);

        assert!(errors.is_empty());
        assert_eq!(changed, [COLOUR_ICONS_KEY]);
        assert!(!appearance.colour_icons());

        let (_, unchanged) = appearance.update_keys(config, &[COLOUR_ICONS_KEY]);
        assert!(unchanged.is_empty());
        let _ = std::fs::remove_dir_all(path);
    }
}
