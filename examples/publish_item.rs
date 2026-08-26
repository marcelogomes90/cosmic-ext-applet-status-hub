use cosmic_status_hub::testkit::{ItemBehaviour, ItemHandle};

#[tokio::main]
async fn main() -> zbus::Result<()> {
    cosmic_status_hub::init_tracing();

    let mut args = std::env::args().skip(1);
    let id = args.next().unwrap_or_else(|| "test-item".to_owned());
    let behaviour = match args.next().as_deref() {
        Some("hangs") => ItemBehaviour::Hangs,
        Some("broken") => ItemBehaviour::Broken,
        _ => ItemBehaviour::Normal,
    };

    let connection = zbus::Connection::session().await?;
    let item = ItemHandle::publish(&connection, &id, behaviour, None).await?;
    println!("published {id} as {} ({behaviour:?})", item.registration);

    tokio::signal::ctrl_c().await.expect("ctrl-c handler");
    println!("exiting");
    Ok(())
}
