name := 'cosmic-status-hub'
appid := 'io.github.marcelogomes90.CosmicStatusHub'

rootdir := ''
prefix := '/usr'

base-dir := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')

bin-src := cargo-target-dir / 'release' / name
bin-dst := base-dir / 'bin' / name
desktop-dst := base-dir / 'share' / 'applications' / appid + '.desktop'
icon-dst := base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg'

user-base := env('XDG_DATA_HOME', env('HOME') / '.local' / 'share')
user-bin-dst := env('HOME') / '.local' / 'bin' / name
user-desktop-dst := user-base / 'applications' / appid + '.desktop'
user-icon-dst := user-base / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg'

default: build-release

clean:
    cargo clean

build-debug *args:
    cargo build {{args}}

build-release *args: (build-debug '--release' args)

check *args:
    cargo clippy --all-targets --all-features {{args}} -- -D warnings

fmt:
    cargo fmt --all

test *args:
    cargo test --features testkit {{args}}

verify: fmt check test

install:
    install -Dm0755 {{bin-src}} {{bin-dst}}
    install -Dm0644 resources/{{appid}}.desktop {{desktop-dst}}
    install -Dm0644 resources/{{appid}}.svg {{icon-dst}}

uninstall:
    rm -f {{bin-dst}} {{desktop-dst}} {{icon-dst}}

install-user:
    install -Dm0755 {{bin-src}} {{user-bin-dst}}
    install -Dm0644 resources/{{appid}}.svg {{user-icon-dst}}
    mkdir -p "$(dirname {{user-desktop-dst}})"
    sed 's|^Exec=.*|Exec={{user-bin-dst}}|' resources/{{appid}}.desktop > {{user-desktop-dst}}
    chmod 0644 {{user-desktop-dst}}
    @echo "Installed. Add 'Status Hub' in Settings -> Desktop -> Panel -> Applets."

uninstall-user:
    rm -f {{user-bin-dst}} {{user-desktop-dst}} {{user-icon-dst}}
