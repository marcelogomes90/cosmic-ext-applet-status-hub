# Architecture

Status Hub is a StatusNotifierHost: it watches the session bus for tray items, resolves their
properties, and renders them in a COSMIC panel applet. This document covers *why* the seams are
where they are — several exist to defend against a specific failure that tray applications cause in
practice.

## Layout

```
src/
├── core/       the tray itself: D-Bus, lifecycle, ordering. No iced, no libcosmic.
│   ├── host.rs registry.rs lifecycle.rs   discovery and arbitration
│   ├── model.rs ordering.rs menu.rs       wire types, stable order, dbusmenu
│   └── call.rs proxies.rs icons.rs        timeouts, zbus proxies, icon options
├── applet/     the COSMIC applet: iced views and presentation state
│   ├── icons/  mod.rs (lookup + cache), paint.rs (raster), svg.rs (vector)
│   ├── pins.rs order.rs identity.rs       panel pins, drag order, window matching
│   └── popup.rs menu_view.rs wayland.rs   surfaces, menus, privileged socket
├── testkit/    fakes that run on a real throwaway bus (feature = "testkit")
├── flatpak.rs  recovering icon paths that belong to another sandbox
└── bin/        cosmic-status-hub-dump, a headless tray dumper
```

## The flow

```
org.kde.StatusNotifierWatcher  (D-Bus)
        │  RegisterStatusNotifierHost, item registered/unregistered signals
        ▼
  core::host          connect / try_activate / resolve_address
        │  Event::{Registered, Unregistered, NameLost, Resolved, Changed, …}
        ▼
  core::Core::run     one tokio::select! actor loop, biased
        │
        ▼
  core::Registry      slots, generations, per-item LifecycleState
        │  Registry::snapshot(watcher)  → ordering::sort_items
        ▼
  TraySnapshot        immutable value, published on a tokio::sync::watch channel
        │  applet::subscription turns the watch channel into an iced Subscription
        ▼
  applet::StatusHub   iced/libcosmic views, popups, icon resolution
```

Everything crossing the boundary in the middle is a plain value. The core never calls into the
applet; it publishes a `TraySnapshot` and an `Option<Arc<MenuModel>>` and lets the UI catch up
whenever it can. The applet talks back only through `CoreCommand` (`src/core/mod.rs`), a small enum
with no reply channel.

## Why `Generation` exists

This is the central invariant, and the reason for most of the shape of `Registry`.

Tray applications restart. A badly behaved one exits without unregistering and reappears a
millisecond later under a new bus name. Meanwhile, property reads issued against the *old* process
are still in flight — 13 concurrent D-Bus calls per resolve, each with its own timeout. Without
arbitration, a reply belonging to a process that no longer exists can overwrite the item that
replaced it, and the tray shows a dead application's title and icon indefinitely.

Two numbers keep that from happening (`src/core/model.rs`):

- **`DiscoverySeq`** identifies a *slot*. Allocated once at discovery, never reused.
- **`Generation`** identifies a *resolve attempt* within a slot. Allocated from a registry-wide
  counter and bumped on every refresh.

Every asynchronous reply carries the `(seq, generation)` it was issued under, and
`Registry::apply_resolved` / `apply_failure` accept it only if **both** still match — otherwise the
reply is dropped as `Stale` (the slot moved on) or `Unknown` (the slot is gone).
`Registry::begin_refresh` bumps the generation *before* issuing new calls, so a refresh invalidates
the resolve it interrupts and no cancellation is required.

`a_reply_from_a_previous_instance_never_touches_its_successor` pins the scenario: one service
discovered under `:1.1`, resolved, its owner lost, reappearing under `:1.2`, and the old reply
replayed against both slots — rejected as `Unknown` by the first and `Stale` by the second. Belt and
braces on purpose: either check alone would cover this case, but the two guard different mistakes,
and only the pair survives a refactor of the other.

## Lifecycle states

`src/core/lifecycle.rs` has no dependencies at all — it is a pure state machine, readable in one
sitting.

| State | Meaning |
| --- | --- |
| `Discovered` | The slot exists; nothing has been asked yet. |
| `Resolving` | A first resolve is in flight. |
| `Ready` | Properties are known and current. |
| `Updating` | A refresh is in flight over properties that are already known. |
| `Degraded { reason }` | The item failed to answer. **Still visible.** |
| `Removing` | The owner is gone. Terminal. |

**`Degraded` does not remove the item.** `is_visible()` excludes only `Removing`. A tray item whose
application has wedged is still an item the user installed and expects to see; hiding it would
misrepresent the system as running fewer applications than it does.
`a_failing_item_stays_visible_and_does_not_hide_the_others` asserts it does not take its neighbours
down with it.

An item only becomes `Degraded` when it fails *all five* identifying properties (`IDENTIFYING` in
`src/core/mod.rs`: `Id`, `Title`, `Status`, `IconName`, `IconPixmap`). A partial answer is treated
as transient and retried on a fixed ladder (`RESOLVE_RETRY_DELAYS`, 250 ms → 45 s), because
applications routinely stall a subset of their properties while starting up.

**`Removing` is terminal and absorbing.** The guard at the top of `LifecycleState::apply` returns
`Removing` for every transition, so no late reply or re-announcement can resurrect an item whose bus
owner has died. `Generation` stops stale data from landing; `Removing` stops a dead slot from
coming back.

## Order and identity

Panel order must not depend on the order replies happened to arrive, or two panels on two monitors
would disagree and a single panel would reshuffle on every restart. `ordering::sort_items` sorts by
`(position in the remembered list, discovery_seq, key)`, and none of the three terms involves
timing: `discovery_seq` is assigned at discovery rather than at resolve, `Registry::entries` is a
`BTreeMap<DiscoverySeq, _>` so iteration starts in discovery order, and the remembered list is a
bounded (`MAX_REMEMBERED = 64`) list of keys persisted through the `OrderStore` trait — written by
`applet/order.rs`, which is where dragging a row in the settings list ends up.
`ordering_is_identical_regardless_of_resolve_order` proves it by resolving three items in three
permutations and asserting one output.

An item's `ItemKey` is its `Id` plus a `dup` index, so two copies of one application are stably
`chat` and `chat#1` rather than swapping places between snapshots. `ItemKey::derive_id` refuses one
`Id`: `chrome_status_icon_<n>`, the placeholder every Chromium and Electron tray publishes unless
the application overrides it. Left alone it collides — 1Password and Chrome would compete for the
same key, and whichever was discovered first would inherit the other's pin. Such an item falls
through to its `Title`, then its tooltip title, which is where those applications put their real
name.

`TrayItem::label` reads the other way round, `Title` first, then the application name inside the
`Id` (`Slack_status_icon_1` → `Slack`), and only then the tooltip. The tooltip is the last resort
because applications use it for status text: Slack publishes "you have 1 notification" there and
qBittorrent publishes its transfer speeds, neither of which is a name.

## Layer boundaries

Read this before moving code between `core/` and `applet/`.

**The core is built on zbus, and that is fine.** Eight of its eleven files use it;
even `model.rs` derives `zvariant::Type` on the wire types. The core *is* the D-Bus layer.

**The core is free of iced and libcosmic, and that is load-bearing.** This is what lets the entire
tray be tested against a real bus without a display server or an iced runtime. The check is
mechanical:

```sh
grep -rn "use crate::" src/core/ | grep -v "use crate::core"   # must print nothing
grep -rn "cosmic::" src/core/                                  # must print nothing
```

**Dependencies point one way:** `applet → core` and `testkit → core`; nothing points back. A
consequence worth stating, because it is a decision rather than an accident: **presentation state
stays out of the core.** Remembered *ordering* lives in the core because it has to be computed where
the snapshot is assembled, alongside duplicate indices and discovery sequences. Anything that is
merely a filter or a preference over an already-built snapshot — pins, drafts — belongs in
`applet/`, even when the core would be marginally more convenient.

## The applet's popup surfaces

The hub and a context menu opened from one of its items share one Wayland surface and one card,
with a divider between them, ordered so the hub content stays nearest the panel. Keeping both in
one `xdg_popup` avoids competing close events and stops the hub button being exposed underneath a
stale child popup.

A menu opened from an item already pinned to the panel is necessarily its own popup, parented to
the panel window and anchored to that item's slot. The `...` button sits at the outer end of the
strip — the end facing away from the screen centre — and pinned items grow inward from it. Which
end that is comes from the panel's own `plugins_wings` and `plugins_center`, read once at startup:
leading for the start wing, trailing for the end wing and for the centre, where a centred block has
no fixed edge and half a button of drift is unavoidable.

The main popup is non-reactive and pin changes never issue a reposition request, so the compositor
cannot reinterpret a stale anchor. They do not need to: the settings view edits a draft, so nothing
reaches the panel while the popup is up. Saving commits and closes in one step; dismissing discards.
The panel is static for as long as anything is anchored to it, which makes the anchor correct by
construction rather than by compensation.

Panel bounds are a maximum on the major axis, not a mandatory size. Each instance shows only the
pinned items that fit its monitor's bounds, always reserving one slot for the hub; the rest stay in
that instance's popup. Any number of items can be pinned — the panel already decides how many it
shows, so a cap on the stored list would only refuse pins the user could reach anyway by unpinning
something else. Pins and icon appearance live in `cosmic_config` and every instance watches them,
so a change on one monitor reaches the others. Both are edited as drafts in the settings view and
committed together through Save.

`HubLayout` computes the body height in Rust, because `Length::Fixed` ignores a container's
intrinsic size: it reserves the divider and at least one pixel of menu space before allocating the
body, and the menu scrolls within what is left.

## Icon resolution

Which property is read depends on the kind and the item's status: an overlay reads `OverlayIcon*`,
a `NeedsAttention` item reads `AttentionIcon*` — falling back to the ordinary `Icon*` when it
publishes neither an attention name nor a valid attention pixmap — and everything else reads
`IconName`/`IconPixmap`. A name beginning with `/` is an absolute path, not a theme name.

The lookup runs in a fixed order (`applet/icons/mod.rs`):

1. the name in the user's icon theme, including its progressively shorter name fallbacks, SVG
   preferred over raster;
2. the name under the item's own `IconThemePath`, accepted only if the result really lives there;
3. the absolute path the item published, if the value is a path and the file exists;
4. the raw pixmap, ARGB converted to RGBA, choosing the smallest frame at least as large as the
   target and otherwise the largest available;
5. `application-default`, then `application-x-executable`.

A relative name and an absolute path are mutually exclusive readings of `IconName`, so step 3 is a
separate branch rather than a candidate that can displace a themed name. Completing the global theme
lookup before consulting `IconThemePath` is a deliberate trade: consistency with the theme the user
chose wins, at the cost of a theme carrying a shorter fallback name beating the exact file the
application shipped.

Many applications publish no icon name at all, so step 4 is a common outcome rather than a last
resort. Only step 5 is flagged as a fallback, and that flag drives a retry ladder stretching to
about a minute, for applications that register an item before publishing its icon. The cache is
keyed by `(address, generation, kind, size)`, so a fresh resolve invalidates an item's icon with no
explicit invalidation anywhere; a change of icon theme, panel colours, or the user's colouring
preference clears it outright.

### Reaching another sandbox's artwork

Steps 2 and 3 also resolve paths this process cannot read (`src/flatpak.rs`). An application in a
Flatpak names its icon directory from inside its own sandbox, `/app/...`, or under its
per-application data directory — neither reachable from here, though the same artwork ships in that
application's payload, which is. A path under `<home>/.var/app/<app id>/` names its owner outright;
a `/app/<tail>` path is matched against the payloads of installed applications, and **that match
must be unique or it is refused**, because a tail like `share/icons` is shipped by nearly every
application (30 of 31 on the machine this was measured on) and serving the wrong artwork is worse
than serving none. The branch only runs when the published path does not exist, so it is inert
outside a sandbox.

### Painting

By default every icon is painted to the panel's foreground ink. The tray is a row of glyphs beside
the clock and the battery, not a row of application logos, and an icon left in its published colours
can be legible on one theme and invisible on the other. Users who prefer application artwork can
disable this painting; ordinary raster and vector icons then keep their published colours, while
explicit symbolic icons and generic fallbacks still follow the panel so they remain legible.

A raster is analysed in Oklab (`applet/icons/paint.rs`). Art larger than the panel size is shrunk
first — the tone analysis reads the same shape either way, and a 256×256 pixmap costs about 0.8 ms
to paint instead of 6.4 ms, which matters because painting happens inside `view()`. The lightness
span is measured over the opaque core, the pixels at nine tenths of the artwork's own peak alpha or
above, because antialiasing is coverage rather than content and should not vote on how the artwork
reads. Its fifth and ninety-fifth percentiles define the usable range so isolated highlights or
shadows cannot flatten the rest of the icon.

Below `MIN_LIGHTNESS_SPAN` the artwork is a silhouette and every pixel becomes the ink exactly.
Above it, each pixel keeps its relative lightness, **anchored at the end nearest the ink**: the
lightest tone lands on the ink on a dark panel, the darkest does on a light one, and everything else
travels away from the panel by at most `MAX_TINT_SHIFT`. The direction is the whole point. Shading
centred on the ink runs both ways, so on a dark panel the dark half of the artwork sinks toward the
background and on a light panel the light half washes out — which is exactly the detail the artwork
was drawn to show. Anchoring means no tone can move toward the panel past a fixed budget, and
`TINT_GAIN` keeps most of the original separation within that budget rather than
stretching every icon to fill it. Hue and chroma come from the ink, so a coloured foreground stays
that colour instead of drifting as it lightens. Recolouring never changes alpha, cutouts and
antialiasing included; the resize that follows smooths alpha alone.

A vector is symbolic outright when its name ends in `-symbolic`. Otherwise it is rendered to 32×32
and measured the same way (`applet/icons/svg.rs`): one achromatic ink means symbolic, while detailed
artwork is rendered at twice its requested size and sent through the same Oklab painter as raster
artwork. Rendering rather than reading the markup is deliberate — resvg is the renderer iced already
draws these files with, so the measurement is of what will actually appear, and CSS, dead style
rules, gradients and masks need no parser of our own. A file whose markup embeds a raster is not
inferred as symbolic; a file that draws nothing is treated as a single ink.

A compact chromatic region touching an image edge is treated as a badge. Its mask is the intersection
of the row and column spans of its coloured pixels rather than a rectangular bounding box, so it
does not consume nearby pixels from the icon body. A separate analysis mask extends one pixel around
the badge so its neutral outline and antialiasing cannot distort the tone profile of the icon body;
that margin is not painted as part of the badge. Badge pixels keep eighty percent of their published
Oklab colour and receive twenty percent of the panel ink, retaining the accent while its body and
edge read as part of the themed icon.

## Raising the window a tray item stands for

The SNI spec has no way to say "show your window". A host calls `Activate` and the application is
expected to raise itself. Under Wayland it cannot: focus arrives only with an xdg-activation token
handed over by whoever owns the input event, and a minimized toplevel cannot unminimize itself at
all. The KDE extension for this is `ProvideXdgActivationToken`, which the applet calls before every
`Activate` and every menu `Event(clicked)`. That is the whole of what Plasma does. The design intent
is that an application cannot take focus, only receive it — so the application, not the host,
decides whether a given menu entry should show a window.

Chromium and Electron trays do not implement the method. They drop the token and the click appears
to do nothing, so for those the applet raises the window itself. `core::resolve` introspects each
item alongside its properties; an interface listing `ProvideXdgActivationToken` is left to decide
for itself. A sandboxed application whose `xdg-dbus-proxy` answers `<node/>` reads as not taking the
token and falls into the rescue, which is the harmless direction.

The rescue runs over the panel's privileged Wayland socket (`X_PRIVILEGED_WAYLAND_SOCKET`), which
cosmic-panel creates through `wp_security_context_v1` with `sandbox_engine =
com.system76.CosmicPanel`; cosmic-comp exposes `zcosmic_toplevel_info_v1` and
`zcosmic_toplevel_manager_v1` to exactly those clients. `applet/wayland.rs` owns that connection and
serves both the activation tokens and the toplevel list from one event loop — the socket is a single
file descriptor and only one thread can hold it.

A click records which matching toplevels exist and whether they are minimized, waits `SETTLE`, then
decides:

| after the click | what happens |
| --- | --- |
| a matching toplevel appeared, or left the minimized state | raise it |
| one entered the minimized state, or closed | nothing — the application handled the click |
| nothing changed | the `Raise` level on the request decides |

`Raise` follows capability, never the wording of a menu entry:

- **left click** → `Raise::Unfocused`, pulling forward even a window merely out of focus.
- **menu entry, item takes the token** → no request at all.
- **menu entry, item does not** → `Raise::Minimized`. The application had no way to act.
- **submenu, or an entry carrying a `toggle-type`** → `Raise::Changed`, which acts on the first two
  rows alone. A toggle is a setting rather than navigation; that is a protocol field, not a reading
  of the label.

Reading the label was tried twice and failed both ways round: an allow-list of show verbs held back
"Show/Hide" and "Biblioteca", a deny-list of action verbs let nearly everything through. dbusmenu
carries nothing that separates them — OBS publishes "Iniciar gravação" with exactly the fields Steam
publishes "Biblioteca" with.

Matching an item to a toplevel is a heuristic (`applet/identity.rs`): the item's `Id`, `Title`,
tooltip title and icon name are split into segments, generic words dropped, and what survives scored
against each toplevel's `app_id` and `title`. The process id would be exact but is useless here —
for a Flatpak application `GetConnectionUnixProcessID` reports its `xdg-dbus-proxy`.

## Failure handling, briefly

- **Per-call timeouts.** Every remote call is wrapped in `with_timeout` (`src/core/call.rs`), so one
  hung application cannot delay another, the popup, or the panel.
- **Per-item error budget.** Retries are per item and bounded by `RESOLVE_RETRY_DELAYS`.
- **Watcher death is survivable.** Losing `org.kde.StatusNotifierWatcher` sets
  `WatcherState::Unavailable` and arms a jittered `Backoff` (seeded from the process id so multiple
  panels do not stampede), but does **not** drop the items.
- **Signals are subscribed before state is read.** `host::connect` subscribes to the registered and
  unregistered streams *before* calling `RegisterStatusNotifierHost` and *before* reading
  `RegisteredStatusNotifierItems`, so no registration slips through the gap between the two.

## Packaging

The Flatpak is Wayland-only, so it does not share the host IPC namespace (that permission is for X11
shared memory). DRI stays for iced/wgpu rendering, and the session bus cannot be narrowed to a fixed
list because a StatusNotifierHost must receive registrations from, and call items under, arbitrary
application bus names.

The read-only filesystem grants are only what icon resolution cannot reach otherwise. Flatpak
already exposes the host's system and user icon themes on `XDG_DATA_DIRS`, at `/run/host/share` and
`/run/host/user-share`, without any permission at all, so nothing asks for those. What it leaves out
is `<installation>/exports/share/icons`, where applications publish the icons they name;
`extend_data_dirs` (`src/lib.rs`) appends those trees to the value Flatpak set rather than replacing
it, which is what keeps the free host themes in the search path. Each entry there is a symlink into
`<installation>/app/<id>/current/active/export`, so the app tree is granted alongside them or every
link dangles. `~/.icons` is granted because that legacy path is a real search root nothing else
covers. The only writable grant is this applet's own COSMIC configuration directory.

Two rules keep the offline build working:

**Do not pin a `rev` on the libcosmic dependency.** `cosmic-panel-config` reaches `cosmic-config`
through the bare libcosmic URL, and a `rev` makes that a second, distinct Cargo source for the same
repository. `flatpak-cargo-generator` emits one source replacement per URL, so the offline build
then cannot resolve the unpinned copy. `Cargo.lock` already pins the exact commit, so builds stay
reproducible without it.

**Regenerate `cargo-sources.json` whenever `Cargo.lock` gains a package** — `just flatpak-sources`.
Adding a dependency that is already in the tree does not: the generator enumerates the packages in
the lock file, so a new edge to a crate that is already vendored changes nothing.

## How to test

`src/testkit/` is not a mock layer. `PrivateBus` spawns a real `dbus-daemon` on a throwaway address
and the fakes are real D-Bus services on it, so the integration tests exercise the actual zbus code
paths.

| Fake | What it is for |
| --- | --- |
| `FakeWatcher` / `FakeCosmicWatcher` | The watcher side, including the real-world quirk where a service name arriving as an object path has to be recombined with the sender. |
| `FakeItem` + `ItemBehaviour` | Misbehaving applications: `Hangs`, `Broken`, `PartlyStalls`, `ItemIsMenu`, `NoMenu`, `NoPrimaryAction`. |
| `FakeMenu` + `MenuBehaviour` | DBusMenu edge cases: `MalformedProperty`, `Empty`, `Submenu`, `SlowAnnouncement`. |

```sh
just test      # cargo test --features testkit
just verify    # fmt-check + clippy -D warnings + test + metadata validation
just run-dump  # headless: dump the live session tray
```

Unit tests live at the bottom of the module they cover, in a `#[cfg(test)] mod tests`, and are named
as sentences describing the invariant (`a_refresh_supersedes_the_resolve_it_interrupts`) rather than
after the function under test. `applet/icons/` shares its fixtures through `icons/testing.rs`.
Integration tests live in `tests/` and share `tests/common/mod.rs`, whose `wait_for` helper renders
the current snapshot into the panic message on timeout.
