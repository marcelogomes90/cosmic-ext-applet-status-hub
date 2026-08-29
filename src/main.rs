use cosmic_status_hub::core::{self, MemoryOrderStore};

fn main() -> cosmic::iced::Result {
    cosmic_status_hub::init_tracing();
    cosmic_status_hub::i18n::init();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "starting cosmic-status-hub"
    );

    cosmic_status_hub::drop_unusable_privileged_socket();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("status-hub-dbus")
        .enable_all()
        .build()
        .expect("failed to start the D-Bus runtime");

    let (handle, _join) = core::spawn(runtime.handle(), MemoryOrderStore::default());

    let _runtime = Box::leak(Box::new(runtime));

    cosmic_status_hub::applet::run(handle)
}
