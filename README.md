# cosmic-status-hub

A StatusNotifierItem tray for the COSMIC desktop, behind a single panel button.

The panel shows one chevron button, pointing at wherever its popup will appear and flipping while it
is open. Clicking it opens a compact popup with the tray items, wrapping them onto additional rows
when necessary.
Nothing else is added to the panel, and the popup is not a settings screen.

This is an independent implementation, written from the StatusNotifierItem and DBusMenu
specifications. It is not a fork of COSMIC's Status Area, and it is built to run alongside it.

## What it is for

The tray is the part of a desktop most exposed to badly behaved applications: they hang, they exit
without warning, they answer with the wrong types, they restart under a new bus name a millisecond
after the old one vanished. The design here is organised around not letting any of that become the
user's problem.

- An item exists exactly as long as its bus owner exists. Removal is driven by `NameOwnerChanged`,
  not by an application remembering to unregister, so there are no leftover icons.
- Every remote call has a timeout, and every item has its own error budget. One wedged application
  cannot delay another, or the popup, or the panel.
- Every asynchronous reply carries a generation token. A reply belonging to a process that has
  already exited can never update its successor.
- Ordering is a pure function of discovery order and a small remembered list, never of the order in
  which futures happened to finish. Every panel shows the same order, and it survives restarts.
- The applet dies with the panel and is relaunched by it, so it rebuilds its entire state on every
  start. Restarting the panel never requires restarting a tray application.

## Layout

```
src/core/     the protocol half: D-Bus, registry, lifecycle, ordering, icons.
              no toolkit dependency, testable against a private bus with no desktop running.
src/applet/   the presentation half: renders published snapshots, sends back user intent.
src/testkit/  fake watcher and fake items, behind the `testkit` feature.
```

The core publishes immutable snapshots and accepts commands. No user-interface code ever awaits a
remote application.

## Building and installing

Needs a Rust toolchain and, for the tests, `dbus-daemon` on `PATH`.

```sh
just build-release
just install-user      # ~/.local, no root
# or
sudo just install      # /usr
```

Then add **Status Hub** in Settings → Desktop → Panel → Applets.

## Development

```sh
just verify            # fmt, clippy with -D warnings, and the full test suite
just test              # unit tests plus integration tests against a private D-Bus session
```

Two binaries help when something looks wrong:

```sh
cosmic-status-hub-dump                                       # the core with no UI, printing every snapshot
cargo run --features testkit --example publish_item -- steam # a synthetic tray item to test against
```

## Limitations

Documented in [docs/limitations.md](docs/limitations.md), including how this applet coexists with
COSMIC's own Status Area and why `Activate` receives no coordinates.

## Licence

GPL-3.0-only.
