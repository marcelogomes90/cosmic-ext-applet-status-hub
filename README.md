<div align="center">

<img src="resources/io.github.marcelogomes90.cosmic-ext-applet-status-hub.svg" width="128" alt="Status Hub icon" />

# Status Hub

A StatusNotifierItem tray for the [COSMIC](https://system76.com/cosmic) desktop, gathered behind
a single panel button.

</div>

The panel shows one button. Clicking it opens a compact popup holding the tray items, wrapping
them onto additional rows when there are many. Items you reach for often can be pinned beside it,
so they stay one click away.

<img src="resources/screenshots/desktop.png" alt="Status Hub popup open on the COSMIC panel, showing pinned items beside the hub button" />

## Features

- Collects every StatusNotifierItem on the session bus behind one panel button.
- Pins the items you choose to the panel, remembered between sessions and kept in step across
  every panel. Pinned items keep their mouse controls and their menu.
- Keeps a popup item's menu in the same card, separated from the icons by a divider; menus opened
  from pinned items are anchored to their panel slot.
- Ships in English, Brazilian Portuguese, Dutch, French, German, Italian, Russian, Simplified
  Chinese, Spanish, and Ukrainian.
- Renders DBusMenu menus, including submenus, checkmarks, radio groups, separators, and menu
  icons.
- Resolves icons from icon names, absolute paths, or raw pixmap bytes, with attention and
  overlay icon support.
- Lets you choose whether application icons keep their published colours or follow the panel theme.
- Uses the standard tray mouse controls: left click activates the application, middle click
  requests its optional secondary action, and right click opens its menu. Activation requests
  carry an XDG activation token so the target application can raise its window.
- Removes an item as soon as its bus owner disappears, so no icon is left stranded when an
  application exits without unregistering.
- Applies a timeout and a per-item error budget to every remote call, so one unresponsive
  application cannot delay another, the popup, or the panel.
- Follows the panel's anchor, size, and theme, and wraps items onto additional rows as they are
  added.

## Installation

### Flatpak

Status Hub is currently being submitted to the official COSMIC Flatpak repository. Direct installation from the COSMIC repository will be available once the submission is accepted.

In the meantime, you can build and install the Flatpak locally:

```sh
git clone https://github.com/marcelogomes90/cosmic-ext-applet-status-hub.git
cd cosmic-ext-applet-status-hub

flatpak-builder --user --install --force-clean build-dir \
  flatpak/io.github.marcelogomes90.cosmic-ext-applet-status-hub/io.github.marcelogomes90.cosmic-ext-applet-status-hub.json
```

This requires `flatpak-builder` and the required Flatpak runtimes.

### From source

Needs a Rust toolchain and the COSMIC development dependencies.

```sh
just build-release
just install-user      # ~/.local, no root
# or
sudo just install      # /usr
```

Then add **Status Hub** in Settings → Desktop → Panel → Applets.

## Contributing

[ARCHITECTURE.md](ARCHITECTURE.md) explains how the tray core, the lifecycle arbitration, and the
applet fit together, and why the core carries no iced dependency. Read it before moving code across
that boundary.

```sh
just verify   # fmt, clippy -D warnings, tests, and metadata validation
```

### Translating

Translations are [Fluent](https://projectfluent.org) catalogues under `i18n/<locale>/status-hub.ftl`.
To add a language, copy `i18n/en/status-hub.ftl` into a new locale directory and translate the
values — the keys must stay as they are. A test asserts every catalogue carries exactly the same
keys as the English one, so a drifting translation fails the build rather than shipping as a blank
label.

## Licence

GPL-3.0-only. See [LICENSE](LICENSE).
