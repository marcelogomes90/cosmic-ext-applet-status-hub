pub mod applet;
pub mod core;

#[cfg(feature = "testkit")]
pub mod testkit;

pub const APP_ID: &str = "io.github.marcelogomes90.CosmicStatusHub";

pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,cosmic_status_hub=info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
