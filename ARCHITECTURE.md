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
│   ├── icons/  mod.rs (lookup + cache), paint.rs (classification + tint), raster.rs (preparation), svg.rs (rendering)
│   ├── pins.rs order.rs identity.rs       panel pins, drag order, window matching
│   └── popup.rs menu_view.rs wayland.rs   surfaces, menus, privileged socket
├── testkit/    fakes that run on a real throwaway bus (feature = "testkit")
├── flatpak.rs  recovering icon paths that belong to another sandbox
└── bin/        cosmic-ext-applet-status-hub-dump, a headless tray dumper
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
   2× logical target and otherwise the largest available, comparing the longest side;
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

The cache also resolves `OverlayIcon*` at half the primary icon's logical size. Overlays keep
their published colours, except explicitly symbolic icons which use the panel ink. Missing or
malformed overlay files fall through to a published pixmap, never a generic placeholder. Missing
overlays are cached until the next item generation; they do not start the primary fallback retry
ladder. Panel buttons, popup items, settings rows and drag previews all use the same composition:
the primary icon with a half-size overlay at the bottom right. Each layer fits proportionally
inside its square, preserving the artwork's aspect ratio.

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

### Preparing and painting icons

Image preparation does not depend on the colouring preference or painting eligibility. SVGs are
rendered once with resvg at twice the logical size, retaining their aspect ratio. PNGs and SNI
pixmaps are decoded at source resolution. All successfully decoded artwork goes through the same
RGBA pipeline: optional painting, then reduction to at most twice the logical size. Smaller rasters
are not enlarged. Catmull-Rom filtering uses premultiplied alpha, and presentation uses proportional
containment. `applet/icons/raster.rs` owns loading, validation, alpha conversion and resizing;
`svg.rs` only rasterizes vectors.

The existing colouring preference enables conservative automatic classification in
`applet/icons/paint.rs`. Only neutral monotones and simple duotones qualify; coloured artwork,
complex contours and uncertain classifications keep their original pixels before resizing. A
single blue or red ink is still coloured artwork. Explicit `-symbolic` names and generic fallbacks
always follow the theme, including when automatic painting is disabled. Successfully prepared
symbolics contain their final colours in RGBA and must not receive another renderer-side tint.
Artwork that cannot be prepared keeps its original handle; malformed overlays fall through to
their pixmap or remain absent, never becoming a generic placeholder.

Classification happens before raster reduction and is independent of the theme. Visible samples
have alpha at least 16, including translucent outlines. A channel spread of at most 16 is neutral;
intensity is integer RGB luminance with weights 54/183/19 over 256. A span at most 24 is monotone.
Otherwise, at least 98% of the alpha mass must be within 12 levels of the two extremes to qualify
directly as a duotone. If this fails, locally solid 3×3 neighbourhoods identify the actual inks.
At least 98% of that solid mass must still fit one or two tones. Remaining samples can count as
antialiasing only within two source pixels of both inks, or of an ink and transparency for a
monotone. The resulting supported samples must cover at least 98% of the full alpha mass.
This admits thin smoothed transitions without admitting broad gradients or unrelated shading.

Before this antialiasing allowance, scanlines in both axes protect contours using all visible
samples, including translucent outlines. Transparency and the badge analysis mask break runs;
intermediate antialiasing samples do not add transitions. A run is complex when it has at least
three transitions, or when a secondary tone surrounds the dominant tone with a separation
greater than 48. An icon stays original if complex runs occur on more than 20% of the visible
scanlines in either axis. A simple detail inside the dominant fill is not itself a border,
and isolated intersections do not reject an otherwise simple drawing. Transparent padding does
not dilute this ratio. These fixed heuristics intentionally favour preservation in uncertain cases.

Monotones receive the foreground ink exactly. In duotones the endpoint with more alpha mass gets
the foreground; ties favour the lighter endpoint. The other endpoint receives 70% foreground and
30% background, mixed directly in RGB with integer rounding. Intermediate intensities interpolate
between these colours. There is no Oklab profile, adaptive contrast search or theme-dependent
classification. Foreground and background remain part of cache invalidation. Painting keeps
alpha exactly, and transparent pixels remain untouched; only subsequent resampling can change
coverage.

A compact chromatic region touching the visible artwork's edge is treated as an embedded badge.
Transparent padding does not affect this edge test. The preserved mask is the intersection of
row and column spans of coloured pixels, including neutral details enclosed by those spans.
Its analysis mask extends one pixel to exclude the badge's outline and antialiasing from base
classification; that extra margin is not part of the preserved mask. Detected badge pixels keep
their original RGBA, including when an explicit symbolic base is painted. A coloured badge does
not disqualify a neutral base. Detection remains a heuristic because a published bitmap does not
identify notification pixels semantically. Separate SNI overlays retain their independent
composition and colour policy.

The internal painting decision records original-with-reason, symbolic, monotone or duotone.
Resolution logs report that decision separately from whether a raster handle was prepared;
successful rasterization alone no longer means an icon was recoloured.

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
covers.

`~/.config/cosmic` is the one read-write grant, and it has to be. `cosmic_config::Config::new`
calls `create_dir_all` on `<id>/v<n>` before it reads a single key, so under a read-only view a
directory the host has not created yet is a hard error — and libcosmic answers that by falling back
to its built-in theme, silently, without a log line. A host whose COSMIC predates the theme config's
`v1` → `v2` bump never creates `com.system76.CosmicTheme.Dark/v2`, so the applet would render with
stock colours and corner radii while every other application looked right. The `dbus-config` feature
does not change this: the settings daemon carries change notifications, the values themselves are
read off the filesystem. Every other COSMIC Flatpak grants this directory read-write.

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
