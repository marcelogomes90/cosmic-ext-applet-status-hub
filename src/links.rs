pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
pub const ISSUES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues");
pub const DEVELOPER: &str = "https://github.com/marcelogomes90";

pub async fn open(url: &'static str) {
    match tokio::process::Command::new("xdg-open").arg(url).spawn() {
        Ok(mut handler) => {
            if let Err(error) = handler.wait().await {
                tracing::warn!(%error, url, "the link handler failed");
            }
        }
        Err(error) => tracing::warn!(%error, url, "could not reach a link handler"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_link_points_at_a_secure_github_page() {
        for url in [REPOSITORY, ISSUES, DEVELOPER] {
            assert!(
                url.starts_with("https://github.com/"),
                "{url} is not an https github address"
            );
        }
    }

    #[test]
    fn the_project_links_follow_the_repository_recorded_in_the_manifest() {
        assert_eq!(ISSUES, format!("{REPOSITORY}/issues"));
        assert!(REPOSITORY.starts_with(DEVELOPER));
    }
}
