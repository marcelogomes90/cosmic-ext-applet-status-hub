use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

pub mod applet;
pub mod core;
pub mod i18n;

#[cfg(feature = "testkit")]
pub mod testkit;

pub const APP_ID: &str = "io.github.marcelogomes90.cosmic-ext-applet-status-hub";

pub fn extend_data_dirs() {
    let current = std::env::var_os("XDG_DATA_DIRS").unwrap_or_default();
    let Some(extended) = data_dirs_with(&current, export_roots()) else {
        return;
    };
    unsafe { std::env::set_var("XDG_DATA_DIRS", extended) };
}

fn export_roots() -> impl Iterator<Item = PathBuf> {
    std::iter::once(PathBuf::from("/var/lib/flatpak"))
        .chain(std::env::home_dir().map(|home| home.join(".local/share/flatpak")))
        .map(|install| install.join("exports/share"))
        .filter(|root| root.is_dir())
}

fn data_dirs_with(current: &OsStr, roots: impl Iterator<Item = PathBuf>) -> Option<OsString> {
    let mut dirs: Vec<PathBuf> = std::env::split_paths(current).collect();
    let listed = dirs.len();

    for root in roots {
        if !dirs.contains(&root) {
            dirs.push(root);
        }
    }

    (dirs.len() > listed)
        .then(|| std::env::join_paths(&dirs).ok())
        .flatten()
}

pub fn drop_unusable_privileged_socket() {
    const VAR: &str = "X_PRIVILEGED_WAYLAND_SOCKET";

    let Ok(raw) = std::env::var(VAR) else {
        return;
    };
    let usable = raw.parse::<u32>().is_ok_and(|fd| {
        std::fs::read_link(format!("/proc/self/fd/{fd}"))
            .is_ok_and(|target| target.to_string_lossy().starts_with("socket:"))
    });
    if usable {
        tracing::info!(socket = %raw, "using the panel's privileged Wayland socket");
        return;
    }

    tracing::warn!(socket = %raw, "the privileged Wayland socket is not usable");
    unsafe { std::env::remove_var(VAR) };
}

pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,cosmic_status_hub=info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEM_EXPORTS: &str = "/var/lib/flatpak/exports/share";
    const USER_EXPORTS: &str = "/home/someone/.local/share/flatpak/exports/share";

    fn joined<const N: usize>(dirs: [&str; N]) -> OsString {
        std::env::join_paths(dirs).expect("the test paths hold no separator")
    }

    fn roots<const N: usize>(dirs: [&str; N]) -> impl Iterator<Item = PathBuf> {
        dirs.map(PathBuf::from).into_iter()
    }

    #[test]
    fn export_roots_are_appended_after_what_flatpak_already_listed() {
        let current = joined([
            "/app/share",
            "/usr/share",
            "/run/host/user-share",
            "/run/host/share",
        ]);

        let extended = data_dirs_with(&current, roots([SYSTEM_EXPORTS, USER_EXPORTS]))
            .expect("neither export tree was listed");

        assert_eq!(
            extended,
            joined([
                "/app/share",
                "/usr/share",
                "/run/host/user-share",
                "/run/host/share",
                SYSTEM_EXPORTS,
                USER_EXPORTS,
            ]),
            "the host theme roots keep their place ahead of the export trees"
        );
    }

    #[test]
    fn a_root_the_session_already_lists_is_not_repeated() {
        let current = joined(["/usr/share", SYSTEM_EXPORTS]);

        let extended = data_dirs_with(&current, roots([SYSTEM_EXPORTS, USER_EXPORTS]))
            .expect("the user export tree was not listed");

        assert_eq!(
            extended,
            joined(["/usr/share", SYSTEM_EXPORTS, USER_EXPORTS])
        );
    }

    #[test]
    fn nothing_is_written_when_every_root_is_already_listed() {
        let current = joined(["/usr/share", SYSTEM_EXPORTS]);

        assert!(data_dirs_with(&current, roots([SYSTEM_EXPORTS])).is_none());
    }
}
