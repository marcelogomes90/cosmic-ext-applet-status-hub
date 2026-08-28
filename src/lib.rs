pub mod applet;
pub mod core;

#[cfg(feature = "testkit")]
pub mod testkit;

pub const APP_ID: &str = "io.github.marcelogomes90.cosmic-ext-applet-status-hub";

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
