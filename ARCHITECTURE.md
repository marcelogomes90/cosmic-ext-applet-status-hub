# Architecture

Status Hub is a StatusNotifierHost: it watches the session bus for tray items, resolves their
properties, and renders them in a COSMIC panel applet. This document describes how the pieces fit
together and, more importantly, *why* the seams are where they are — several of them exist to
defend against a specific failure that tray applications cause in practice.

## Layout

```
src/
├── core/       the tray itself: D-Bus, lifecycle, ordering. No iced, no libcosmic.
├── applet/     the COSMIC applet: iced views, popups, icon cache, presentation state,
│            and the privileged Wayland connection for tokens and window raising.
├── testkit/    fakes that run on a real throwaway bus (feature = "testkit").
├── i18n.rs     Fluent catalogue loading.
└── bin/        cosmic-status-hub-dump, a headless tray dumper.
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
whenever it can. The applet talks back only through `CoreCommand` (`src/core/mod.rs`), a small
enum with no reply channel.

## Why `Generation` exists

This is the central invariant, and the reason for most of the shape of `Registry`.

Tray applications restart. A badly behaved one exits without unregistering and reappears a
millisecond later under a new bus name. Meanwhile, property reads issued against the *old* process
are still in flight — 13 concurrent D-Bus calls per resolve, each with its own timeout. Without
arbitration, a reply belonging to a process that no longer exists can overwrite the item that
replaced it, and the tray shows a dead application's title and icon indefinitely.

Two numbers keep that from happening (`src/core/model.rs`):

- **`DiscoverySeq`** identifies a *slot*. It is allocated once when an item is discovered and is
  never reused, even after the item is removed.
- **`Generation`** identifies a *resolve attempt* within a slot. It is allocated from a
  registry-wide counter and bumped on every refresh.

Every asynchronous reply carries the `(seq, generation)` it was issued under, and
`Registry::apply_resolved` / `apply_failure` accept it only if **both** still match. The return
value says which check rejected it:

| `Applied` | Meaning |
| --- | --- |
| `Changed` | Applied. The registry revision advances and a new snapshot is published. |
| `Stale` | The slot exists, but has moved on to a newer generation. Dropped. |
| `Unknown` | The slot is gone entirely. Dropped. |

`Registry::begin_refresh` bumps the generation *before* issuing new calls, so a refresh
automatically invalidates the resolve it interrupts — no cancellation is required, and an in-flight
reply that arrives late is simply `Stale`.

The concrete scenario is pinned by
`a_reply_from_a_previous_instance_never_touches_its_successor` in `src/core/registry.rs`. It
discovers `org.example.A` owned by `:1.1`, resolves it, loses the owner, discovers the same service
now owned by `:1.2`, and resolves that. Then it replays the *old* reply and asserts both defences
independently:

```rust
// The old slot is gone.
assert_eq!(registry.apply_resolved(seq_a, gen_a, props("first")), Applied::Unknown);
// And even if that reply were misrouted to the new slot, its generation is too old.
assert_eq!(registry.apply_resolved(seq_b, gen_a, props("first")), Applied::Stale);
```

Belt and braces on purpose: either check alone would be enough for this case, but the two guard
different mistakes, and only the pair survives a future refactor of the other.

## Lifecycle states

`src/core/lifecycle.rs` is deliberately tiny and has no dependencies at all — it is pure state
machine, so its rules can be read in one sitting.

| State | Meaning |
| --- | --- |
| `Discovered` | The slot exists; nothing has been asked yet. |
| `Resolving` | A first resolve is in flight. |
| `Ready` | Properties are known and current. |
| `Updating` | A refresh is in flight over properties that are already known. |
| `Degraded { reason }` | The item failed to answer. **Still visible.** |
| `Removing` | The owner is gone. Terminal. |

Two rules matter more than the rest:

**`Degraded` does not remove the item.** `is_visible()` excludes only `Removing`. A tray item whose
application has wedged is still an item the user installed and expects to see; hiding it would
silently misrepresent the system as having fewer applications running than it does. So a failing
item stays in the tray, and `a_failing_item_stays_visible_and_does_not_hide_the_others` asserts that
it does not take its neighbours down with it.

An item only becomes `Degraded` when it fails to answer *all five* identifying properties (`Id`,
`Title`, `Status`, `IconName`, `IconPixmap` — see `IDENTIFYING` and `answered` in
`src/core/mod.rs`). A partial answer is treated as transient and retried on a fixed ladder
(`RESOLVE_RETRY_DELAYS`, 250 ms → 45 s), because applications routinely stall a subset of their
properties while starting up.

**`Removing` is terminal and absorbing.** The guard at the top of `LifecycleState::apply` returns
`Removing` for every transition, so no late reply, refresh, or re-announcement can resurrect an item
whose bus owner has died. This is the second half of the restart defence: `Generation` stops stale
data from landing, and `Removing` stops a dead slot from coming back.

## Ordering is a pure function

Panel order must not depend on the order replies happened to arrive, or two panels on two monitors
would disagree, and a single panel would reshuffle itself on every restart.

`ordering::sort_items` sorts by the tuple `(position in the remembered list, discovery_seq, key)`.
None of the three terms involves timing:

- `discovery_seq` is monotonic and assigned at discovery, not at resolve.
- `Registry::entries` is a `BTreeMap<DiscoverySeq, _>`, so iteration is already in discovery order
  before sorting begins.
- The remembered list is a bounded (`MAX_REMEMBERED = 64`) list of `ItemKey`s persisted through the
  `OrderStore` trait.

`ordering_is_identical_regardless_of_resolve_order` proves this by resolving the same three items in
three different permutations and asserting one output. Duplicate ids get a `dup` index assigned in
discovery order (`assign_dup_indices`), so two instances of the same application are stably
distinguished as `chat` and `chat#1` rather than swapping places between snapshots.

## Layer boundaries

Read this before moving code between `core/` and `applet/`.

**The core is built on zbus, and that is fine.** Eight of the ten files under `src/core/` use it;
even `model.rs` derives `zvariant::Type` on the wire types and uses `zbus::names` for
`ItemAddress`. The core *is* the D-Bus layer — there is no pretence otherwise.

**The core is completely free of iced and libcosmic, and that is load-bearing.** There is no
`use cosmic::` anywhere under `src/core/`; the only match for "iced" is a comment in `icons.rs`
explaining pixmap byte order. This is what lets the entire tray be tested against a real bus without
constructing a display server or an iced runtime — `cargo test --features testkit` exercises
discovery, lifecycle, ordering, and menus with no compositor in sight.

**Dependencies point one way.** `applet → core` and `testkit → core`; nothing points back. The check
is mechanical:

```sh
grep -rn "use crate::" src/core/ | grep -v "use crate::core"   # must print nothing
grep -rn "cosmic::" src/core/                                  # must print nothing
grep -rni "iced" src/core/                                     # must find only that one comment
```

A consequence worth stating explicitly, because it is a decision rather than an accident:
**presentation state stays out of the core.** Remembered *ordering* lives in the core because it has
to be computed where the snapshot is assembled, alongside duplicate indices and discovery
sequences. Anything that is merely a filter or a preference over an already-built snapshot belongs
in `applet/`, even when putting it in the core would be marginally more convenient. Moving config
handling into the core would erode the boundary above, and the test story goes with it.

## The applet's popup surfaces

The hub and a context menu opened from one of its items share one Wayland surface and one card, with
a divider between them. Their order follows the panel anchor so the hub content always stays nearest
the panel. Keeping everything in one `xdg_popup` avoids competing close events and prevents the hub
button from being exposed underneath a stale child popup.

A menu opened from an item already pinned to the panel is necessarily its own popup, parented to the
panel window and anchored to that item's slot. The `...` button sits at the outer end of the strip,
the end facing away from the screen centre, and pinned items grow inward from it. Which end that is
depends on the group the applet was placed in, read once at startup from the panel's own
`plugins_wings` and `plugins_center`: leading for the start wing, trailing for the end wing and for
the centre, where a centred block has no fixed edge at all and half a button of drift is
unavoidable.

The main popup is non-reactive, so resizing its parent cannot make the compositor reinterpret a
stale anchor, and pin changes never issue a reposition request. They do not need to: the settings
view edits a draft, so nothing reaches the panel while the popup is up. Saving commits the draft and
closes the popup in one step, and dismissing the popup discards it. The panel is therefore static
for as long as anything is anchored to it, which makes the anchor correct by construction instead of
by compensation.

Panel bounds are treated as a maximum on the panel's major axis, not as a mandatory size. Each
instance shows only the pinned items that fit in its monitor's current bounds, always reserving one
slot for the hub; excess pinned items stay available in that instance's popup. This also lets the
autosizer request the smaller natural size as soon as items are unpinned instead of retaining an
empty allocation left by overflow.

The hub body height is decided in Rust from its current rows and padding. `Length::Fixed` ignores a
container's intrinsic size, so `HubLayout` reserves the menu divider and at least one pixel of menu
space before allocating the body. The menu is scrollable within the remaining height budget,
keeping the combined surface under the popup ceiling.

Pinned items are applet state, deliberately kept out of the core (see above). They are stored
through `cosmic_config` and each panel instance watches that config, so pinning on one monitor
reaches the others.

The Flatpak is Wayland-only, so it does not share the host IPC namespace (that permission is for
X11 shared memory). DRI remains available for iced/wgpu rendering, and the session bus cannot be
narrowed to a fixed list because a StatusNotifierHost must receive registrations and call items
under arbitrary application bus names.

The read-only filesystem grants are only what icon resolution cannot reach otherwise. Flatpak
already exposes the host's system and user icon themes on `XDG_DATA_DIRS`, at `/run/host/share`
and `/run/host/user-share`, without any permission at all, so no grant asks for those. What it
leaves out is `<installation>/exports/share/icons`, where applications publish the icons they name;
`extend_data_dirs` (`src/lib.rs`) appends those trees to the value Flatpak set rather than replacing
it, which is what keeps the free host themes in the search path. Each entry there is a symlink into
`<installation>/app/<id>/current/active/export`, so the app tree is granted alongside them or every
link dangles. `~/.icons` is granted because that legacy path is a real search root nothing else
covers. The only writable grant is this applet's own COSMIC configuration directory, where pins are
stored.

## Icon resolution

Which property is read depends on the kind and the item's status: an overlay reads `OverlayIcon*`, a
`NeedsAttention` item reads `AttentionIcon*` — falling back to the ordinary `Icon*` when it publishes
neither an attention name nor a valid attention pixmap — and everything else reads
`IconName`/`IconPixmap`. A name beginning with `/` is treated as an absolute path, not a theme name.

The lookup then runs in a fixed order (`applet/icons.rs`):

1. the name in the user's icon theme, including its progressively shorter name fallbacks, with SVG
   preferred over raster;
2. the name under the item's own `IconThemePath`, accepted only if the result really lives there;
3. the absolute path the item published, if the value is a path and the file exists;
4. the raw pixmap, ARGB converted to RGBA, choosing the smallest frame at least as large as the
   target and otherwise the largest available;
5. `application-default`, then `application-x-executable`.

A relative name and an absolute path are mutually exclusive interpretations of `IconName`, so step
3 is a separate branch rather than a candidate that can displace a themed relative name. Completing
the global theme lookup before consulting `IconThemePath` is deliberate: visual consistency with
the user's chosen theme wins even when the match is a shorter, more generic name.

Steps 2 and 3 also resolve a path this process cannot read (`src/flatpak.rs`). An application in a
Flatpak names its icon directory either from inside its own sandbox, `/app/...`, or under the
per-application data directory, and neither is reachable from here — but the same artwork ships in
that application's payload, which is. The owner is recovered two ways: a path under
`<home>/.var/app/<app id>/` names the application outright, while a `/app/<tail>` path is matched
against the payloads of installed applications. **That match must be unique or it is refused.** A
tail like `share/icons` is shipped by nearly every application — 30 of 31 on the machine this was
measured on — and serving another application's artwork is worse than serving none, so anything but
a single candidate falls through to the next step. Once the owner is known the search covers the
translated tail plus `share/icons` and `share/pixmaps`, the same pair the freedesktop lookup derives
from any data directory. Each candidate is canonicalised first: a payload is reached through the
`current` and `active` symlinks, and the walker canonicalises the roots it is handed, so the guard
that keeps a result inside its own root would otherwise reject every match.

This only runs when the published path does not exist, which leaves every item that resolves today
untouched and makes the whole branch inert outside a sandbox. A raster recovered this way is passed
through the same adaptation as a published pixmap, because the application drew it for its own panel
rather than for this theme; a file found where the application said it would be keeps the appearance
it always had.

Painting is deliberately conservative. An SVG is symbolic when its name ends in `-symbolic`, or
when its live paint rules describe at most one achromatic ink; multi-tone, coloured, embedded, or
unrecognised content keeps its original appearance.

A raster is adapted only when it has a transparent margin and cannot already be read on the panel.
Artwork keeps its published appearance when most of its ink already clears 3:1 against the panel
background, or when a meaningful share does *and* that share traces the whole shape. Both halves are
needed. Judging by the average tone fails where it matters most: a black glyph inside a thick white
outline averages to a mid grey that contrasts with nothing, while both of its real tones read on any
panel. Judging by share alone fails the other way: a white icon with a small dark glyph in the middle
clears the share test while the rest of it disappears, so the legible ink is also required to span
most of the artwork's extent rather than sit as a detail inside it.

Legibility and the representative tone are measured on the opaque core, the pixels at nine tenths of
the artwork's own peak alpha or above, because antialiasing is coverage rather than content and
should not vote on how the artwork reads. Whether the artwork is tonal is decided on every visible
pixel instead: a light disc drawn with a darker outline carries that outline below the core's alpha,
and treating it as single-tone flattens the whole icon into one solid shape.

Single-tone achromatic silhouettes receive the theme foreground directly. Multi-tone achromatic
artwork is translated onto the theme foreground: each tone keeps its own distance from the
alpha-weighted representative tone, capped so no detail runs away, so the published tonal structure
survives at its original width instead of being stretched to fill a fixed range. Alpha is never
changed, including fully transparent cutouts and partially transparent antialiasing.

A compact chromatic region touching an image edge is treated as a possible badge. Its complete
bounding region, plus a small safety margin, is kept untouched (including achromatic text and
antialiasing inside it), but the base outside that region is adapted only when it is single-tone. If
the base itself is multi-tone, changing one part while freezing the badge is too likely to split one
piece of artwork, so the whole pixmap is kept as published. A chromatic region that is large,
dispersed, central, or leaves no independently paintable base is likewise preserved in full. This
lets a plainly separable badge coexist with a symbolic base without interpreting ordinary
multicolour application art as symbolic.

Many applications publish no icon name at all, so step 4 is a common outcome rather than a last
resort. Only step 5 is marked as a fallback, and that flag drives the retry ladder: a fallback entry
is resolved again on a schedule stretching to about a minute, for applications that register an item
before publishing its icon. The cache is keyed by `(address, generation, kind, size)`, so a fresh
resolve of the item invalidates its icon with no explicit invalidation anywhere. A change to the
icon theme, panel background, or foreground also clears the cache so both asset selection and
recoloured pixmaps follow the active theme.

Placing the complete theme lookup ahead of `IconThemePath` is a deliberate trade: it keeps the tray
consistent with the theme the user chose, at the cost of a theme carrying a shorter fallback name
winning over the exact file the application shipped.

## Raising the window a tray item stands for

The SNI spec has no way to say "show your window". A host calls `Activate` and the application is
expected to raise itself. Under Wayland it cannot: focus only arrives with an xdg-activation token
handed over by whoever owns the input event, and a minimized toplevel cannot unminimize itself at
all. The KDE extension for this is `ProvideXdgActivationToken`, which the applet calls before every
`Activate` and every menu `Event(clicked)`.

That is the whole of what Plasma does, and all any tray host does. The design intent is that an
application cannot take focus, only receive it — so the application, not the host, decides whether a
given menu entry should show a window.

Chromium and Electron trays do not implement the method. They drop the token and the click appears
to do nothing, so for those the applet raises the window itself. `core::resolve` introspects each
item alongside its properties; an interface listing `ProvideXdgActivationToken` is left to decide for
itself. A sandboxed application whose `xdg-dbus-proxy` answers `<node/>` reads as not taking the
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

- **left click** → `Raise::Unfocused`, pulling forward even a window that is merely out of focus.
  The request is unambiguous, and it covers a compositor that will not unminimize through
  xdg-activation alone.
- **menu entry, item takes the token** → no request at all.
- **menu entry, item does not** → `Raise::Minimized`. The application had no way to act.
- **submenu, or an entry carrying a `toggle-type`** → `Raise::Changed`, which acts on the first two
  rows alone. A toggle is a setting rather than navigation; that is a protocol field, not a reading
  of the label.

Reading the label was tried twice and failed both ways round: an allow-list of show verbs held back
"Show/Hide" and "Biblioteca", a deny-list of action verbs let nearly everything through. dbusmenu
carries nothing that separates them — OBS publishes "Iniciar gravação" with a label, `enabled` and
`visible` and nothing else, exactly as Steam publishes "Biblioteca".

Matching an item to a toplevel is still a heuristic (`applet/identity.rs`): the item's `Id`,
`Title`, tooltip title and icon name are split into segments, generic words dropped, and what
survives scored against each toplevel's `app_id` and `title`. The process id would be exact but is
useless here — for a Flatpak application `GetConnectionUnixProcessID` reports its `xdg-dbus-proxy`.

## Failure handling, briefly

- **Per-call timeouts.** Every remote call is wrapped in `with_timeout` (`src/core/call.rs`), so one
  hung application cannot delay another, the popup, or the panel.
- **Per-item error budget.** Retries are per item and bounded by `RESOLVE_RETRY_DELAYS`.
- **Watcher death is survivable.** Losing `org.kde.StatusNotifierWatcher` sets
  `WatcherState::Unavailable` and arms a jittered `Backoff` (seeded from the process id so multiple
  panels do not stampede), but does **not** drop the items.
- **Signals are subscribed before state is read.** `host::connect` subscribes to the registered and
  unregistered streams *before* calling `RegisterStatusNotifierHost` and *before* reading
  `RegisteredStatusNotifierItems`, so no registration can slip through the gap between the two.

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
after the function under test. Integration tests live in `tests/` and share `tests/common/mod.rs`,
whose `wait_for` helper renders the current snapshot into the panic message on timeout.
