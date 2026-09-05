use cosmic_ext_applet_status_hub::applet::order::ConfigOrderStore;
use cosmic_ext_applet_status_hub::core;

fn main() -> cosmic::iced::Result {
    cosmic_ext_applet_status_hub::extend_data_dirs();

    cosmic_ext_applet_status_hub::init_tracing();
    cosmic_ext_applet_status_hub::i18n::init();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "starting cosmic-ext-applet-status-hub"
    );

    cosmic_ext_applet_status_hub::drop_unusable_privileged_socket();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("status-hub-dbus")
        .enable_all()
        .build()
        .expect("failed to start the D-Bus runtime");

    let (handle, _join) = core::spawn(
        runtime.handle(),
        ConfigOrderStore::open(cosmic_ext_applet_status_hub::APP_ID),
    );

    let _runtime = Box::leak(Box::new(runtime));

    cosmic_ext_applet_status_hub::applet::run(handle)
}
