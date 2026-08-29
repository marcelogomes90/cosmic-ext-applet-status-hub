use std::sync::LazyLock;

use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::{DefaultLocalizer, LanguageLoader, Localizer};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader = fluent_language_loader!();
    loader
        .load_fallback_language(&Localizations)
        .expect("the fallback language is embedded in the binary");
    loader
});

pub fn init() {
    let localizer = DefaultLocalizer::new(&*LANGUAGE_LOADER, &Localizations);
    let requested = i18n_embed::DesktopLanguageRequester::requested_languages();

    match localizer.select(&requested) {
        Ok(selected) => tracing::info!(?selected, ?requested, "loaded translations"),
        Err(error) => tracing::warn!(%error, "keeping English, the locale did not load"),
    }
}

#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        ::i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id)
    }};
    ($message_id:literal, $($args:expr),*) => {{
        ::i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id, $($args),*)
    }};
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    fn message_ids(catalogue: &str) -> BTreeSet<&str> {
        catalogue
            .lines()
            .filter(|line| !line.starts_with([' ', '\t', '#', '-']))
            .filter_map(|line| line.split_once('='))
            .map(|(id, _)| id.trim())
            .filter(|id| !id.is_empty())
            .collect()
    }

    fn assert_matches_english(language: &str, catalogue: &str) {
        let english = message_ids(include_str!("../i18n/en/status-hub.ftl"));
        let translated = message_ids(catalogue);

        assert!(
            translated == english,
            "{language} is out of step with en: missing {:?}, unknown {:?}",
            english.difference(&translated).collect::<Vec<_>>(),
            translated.difference(&english).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn every_language_translates_exactly_the_same_messages() {
        for (language, catalogue) in [
            ("de", include_str!("../i18n/de/status-hub.ftl")),
            ("es", include_str!("../i18n/es/status-hub.ftl")),
            ("fr", include_str!("../i18n/fr/status-hub.ftl")),
            ("it", include_str!("../i18n/it/status-hub.ftl")),
            ("nl", include_str!("../i18n/nl/status-hub.ftl")),
            ("pt-BR", include_str!("../i18n/pt-BR/status-hub.ftl")),
            ("ru", include_str!("../i18n/ru/status-hub.ftl")),
            ("uk", include_str!("../i18n/uk/status-hub.ftl")),
            ("zh-CN", include_str!("../i18n/zh-CN/status-hub.ftl")),
        ] {
            assert_matches_english(language, catalogue);
        }
    }

    #[test]
    fn attributes_and_comments_are_not_mistaken_for_messages() {
        let ids = message_ids("# a comment = not a message\nreal = yes\n    .tooltip = no\n");

        assert_eq!(ids, BTreeSet::from(["real"]));
    }
}
