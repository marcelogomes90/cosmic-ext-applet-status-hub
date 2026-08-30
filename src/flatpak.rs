use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

const PAYLOAD: &str = "current/active/files";

const SANDBOX: &str = "/app";

const DATA_DIRS: [&str; 2] = ["share/icons", "share/pixmaps"];

pub fn installations() -> impl Iterator<Item = PathBuf> {
    std::iter::once(PathBuf::from("/var/lib/flatpak"))
        .chain(std::env::home_dir().map(|home| home.join(".local/share/flatpak")))
}

pub fn payload_roots(published: &str) -> Vec<PathBuf> {
    payload_roots_in(Path::new(published), &installations().collect::<Vec<_>>())
}

pub fn payload_file(published: &str) -> Option<PathBuf> {
    payload_file_in(Path::new(published), &installations().collect::<Vec<_>>())
}

enum Published<'a> {
    DataDir(&'a str),
    Sandbox(&'a Path),
}

fn payload_roots_in(published: &Path, installations: &[PathBuf]) -> Vec<PathBuf> {
    let Some((payload, tail)) = payload_and_tail(published, installations, Path::is_dir) else {
        return Vec::new();
    };

    let mut roots: Vec<PathBuf> = tail.map(|tail| payload.join(tail)).into_iter().collect();
    roots.extend(DATA_DIRS.iter().map(|data_dir| payload.join(data_dir)));
    roots.retain(|root| root.is_dir());
    roots
}

fn payload_file_in(published: &Path, installations: &[PathBuf]) -> Option<PathBuf> {
    let (payload, tail) = payload_and_tail(published, installations, Path::is_file)?;
    let file = payload.join(tail?);
    file.is_file().then_some(file)
}

fn payload_and_tail<'a>(
    published: &'a Path,
    installations: &[PathBuf],
    accept: fn(&Path) -> bool,
) -> Option<(PathBuf, Option<&'a Path>)> {
    match classify(published)? {
        Published::DataDir(app_id) => Some((payload_of(app_id, installations)?, None)),
        Published::Sandbox(tail) => {
            Some((sole_payload_with(tail, installations, accept)?, Some(tail)))
        }
    }
}

fn classify(published: &Path) -> Option<Published<'_>> {
    if let Ok(tail) = published.strip_prefix(SANDBOX) {
        return (!tail.as_os_str().is_empty()).then_some(Published::Sandbox(tail));
    }
    app_id_of_data_dir(published).map(Published::DataDir)
}

fn app_id_of_data_dir(published: &Path) -> Option<&str> {
    let names: Vec<&OsStr> = published.components().map(Component::as_os_str).collect();
    names
        .windows(3)
        .find(|window| window[0] == ".var" && window[1] == "app")
        .and_then(|window| window[2].to_str())
}

fn payload_of(app_id: &str, installations: &[PathBuf]) -> Option<PathBuf> {
    installations
        .iter()
        .map(|install| install.join("app").join(app_id).join(PAYLOAD))
        .find(|payload| payload.is_dir())
}

fn sole_payload_with(
    tail: &Path,
    installations: &[PathBuf],
    accept: fn(&Path) -> bool,
) -> Option<PathBuf> {
    let mut sole = None;

    for install in installations {
        let Ok(apps) = std::fs::read_dir(install.join("app")) else {
            continue;
        };
        for app in apps.filter_map(Result::ok) {
            let payload = app.path().join(PAYLOAD);
            if !accept(&payload.join(tail)) {
                continue;
            }
            if sole.is_some() {
                return None;
            }
            sole = Some(payload);
        }
    }

    sole
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(root: &Path, app_id: &str) -> PathBuf {
        let app = root.join("app").join(app_id);
        let branch = app.join("x86_64/stable");
        std::fs::create_dir_all(branch.join("0123456789abcdef/files")).unwrap();
        std::os::unix::fs::symlink("x86_64/stable", app.join("current")).unwrap();
        std::os::unix::fs::symlink("0123456789abcdef", branch.join("active")).unwrap();
        app.join(PAYLOAD)
    }

    fn root(suffix: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "status-hub-flatpak-{}-{suffix}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn touch(path: PathBuf) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, []).unwrap();
        path
    }

    #[test]
    fn a_data_directory_path_names_its_own_application() {
        let install_root = root("data-dir");
        let files = install(&install_root, "org.example.Player");
        touch(files.join("share/pixmaps/player_tray.png"));

        let roots = payload_roots_in(
            Path::new("/home/someone/.var/app/org.example.Player/.local/share/Player/public"),
            std::slice::from_ref(&install_root),
        );

        assert_eq!(
            roots,
            vec![files.join("share/pixmaps")],
            "only the data directories the application actually ships are searched"
        );
        std::fs::remove_dir_all(install_root).unwrap();
    }

    #[test]
    fn a_sandbox_path_is_translated_through_the_application_that_ships_it() {
        let install_root = root("sandbox-tail");
        let files = install(&install_root, "org.example.Sync");
        let published = files.join("extra/dist/images");
        touch(published.join("hicolor/16x16/status/sync-idle.png"));

        let roots = payload_roots_in(
            Path::new("/app/extra/dist/images"),
            std::slice::from_ref(&install_root),
        );

        assert_eq!(
            roots,
            vec![published],
            "the translated tail is searched, and the application ships no data directories"
        );
        std::fs::remove_dir_all(install_root).unwrap();
    }

    #[test]
    fn a_tail_two_applications_ship_is_refused() {
        let install_root = root("ambiguous");
        for app_id in ["org.example.One", "org.example.Two"] {
            let files = install(&install_root, app_id);
            touch(files.join("share/icons/hicolor/scalable/apps/shared.svg"));
        }

        assert!(
            payload_roots_in(
                Path::new("/app/share/icons"),
                std::slice::from_ref(&install_root)
            )
            .is_empty(),
            "serving either application's artwork would be worse than serving none"
        );
        std::fs::remove_dir_all(install_root).unwrap();
    }

    #[test]
    fn a_path_belonging_to_no_installed_application_resolves_to_nothing() {
        let install_root = root("absent");
        install(&install_root, "org.example.Present");

        assert!(
            payload_roots_in(
                Path::new("/app/extra/missing"),
                std::slice::from_ref(&install_root)
            )
            .is_empty()
        );
        assert!(
            payload_roots_in(
                Path::new("/home/someone/.var/app/org.example.Absent/data"),
                std::slice::from_ref(&install_root),
            )
            .is_empty()
        );
        assert!(
            payload_roots_in(
                Path::new("/opt/vendor/icons"),
                std::slice::from_ref(&install_root)
            )
            .is_empty(),
            "a path that is neither a sandbox path nor a data directory says nothing"
        );
        std::fs::remove_dir_all(install_root).unwrap();
    }

    #[test]
    fn an_absolute_icon_path_resolves_to_the_payload_file() {
        let install_root = root("file");
        let files = install(&install_root, "org.example.Single");
        let icon = touch(files.join("share/icons/hicolor/scalable/apps/single.svg"));

        assert_eq!(
            payload_file_in(
                Path::new("/app/share/icons/hicolor/scalable/apps/single.svg"),
                std::slice::from_ref(&install_root),
            ),
            Some(icon)
        );
        assert_eq!(
            payload_file_in(
                Path::new("/home/someone/.var/app/org.example.Single/cache/single.svg"),
                std::slice::from_ref(&install_root),
            ),
            None,
            "a file written at runtime has no counterpart in the payload"
        );
        std::fs::remove_dir_all(install_root).unwrap();
    }

    #[test]
    fn a_directory_named_like_the_sandbox_prefix_is_not_a_sandbox_path() {
        let install_root = root("prefix");
        install(&install_root, "org.example.Prefixed");

        assert!(
            payload_roots_in(
                Path::new("/applications/icons"),
                std::slice::from_ref(&install_root)
            )
            .is_empty()
        );
        std::fs::remove_dir_all(install_root).unwrap();
    }
}
