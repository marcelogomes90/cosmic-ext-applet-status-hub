use cosmic_ext_applet_status_hub::core::{self, MemoryOrderStore};

#[tokio::main]
async fn main() {
    cosmic_ext_applet_status_hub::init_tracing();

    let (handle, _join) = core::spawn(
        &tokio::runtime::Handle::current(),
        MemoryOrderStore::default(),
    );
    let mut snapshots = handle.subscribe();

    println!("watching the session tray; press ctrl-c to stop");

    loop {
        tokio::select! {
            result = snapshots.changed() => {
                if result.is_err() {
                    break;
                }
                print_snapshot(&snapshots.borrow_and_update());
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
}

fn print_snapshot(snapshot: &core::model::TraySnapshot) {
    println!(
        "\n--- revision {} | watcher {:?} | {} item(s)",
        snapshot.revision,
        snapshot.watcher,
        snapshot.items.len()
    );
    for (position, item) in snapshot.items.iter().enumerate() {
        println!(
            "{position:>2}. {key:<28} {state:<10} seq={seq:<3} gen={gen:<3} menu={menu} {address}",
            key = item.key.to_string(),
            state = item.state.to_string(),
            seq = item.discovery_seq.0,
            gen = item.generation.0,
            menu = item.menu_path.as_ref().map_or("-", |p| p.as_str()),
            address = item.address,
        );
    }
}
