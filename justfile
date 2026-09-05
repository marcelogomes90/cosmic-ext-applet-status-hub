name := 'cosmic-status-hub'
appid := 'io.github.marcelogomes90.cosmic-ext-applet-status-hub'

rootdir := ''
prefix := '/usr'

base-dir := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')

bin-src := cargo-target-dir / 'release' / name
bin-dst := base-dir / 'bin' / name
desktop-dst := base-dir / 'share' / 'applications' / appid + '.desktop'
metainfo-dst := base-dir / 'share' / 'metainfo' / appid + '.metainfo.xml'
icon-dst := base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg'
symbolic-icon-dst := base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '-symbolic.svg'

user-base := env('XDG_DATA_HOME', env('HOME') / '.local' / 'share')
user-bin-dst := env('HOME') / '.local' / 'bin' / name
user-desktop-dst := user-base / 'applications' / appid + '.desktop'
user-metainfo-dst := user-base / 'metainfo' / appid + '.metainfo.xml'
user-icon-dst := user-base / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg'
user-symbolic-icon-dst := user-base / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '-symbolic.svg'

flatpak-dir := 'flatpak' / appid
manifest := flatpak-dir / appid + '.json'
flatpak-build-dir := 'build' / 'flatpak'

default: build-release

clean:
    cargo clean

clean-flatpak:
    rm -rf build .flatpak-builder

build-debug *args:
    cargo build {{args}}

build-release *args: (build-debug '--release' args)

check *args:
    cargo clippy --all-targets --all-features {{args}} -- -D warnings

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

test *args:
    cargo test --features testkit {{args}}

validate:
    #!/usr/bin/env bash
    set -euo pipefail
    output="$(desktop-file-validate resources/{{appid}}.desktop || true)"
    output="$(printf '%s\n' "${output}" | grep -v 'unregistered value "COSMIC"' \
        | grep -v 'does not contain a registered main category' | grep -v '^$' || true)"
    if [ -n "${output}" ]; then printf '%s\n' "${output}"; exit 1; fi
    echo "desktop entry ok"
    appstreamcli validate --no-net resources/{{appid}}.metainfo.xml

verify: fmt-check check test validate

run *args:
    cargo run --bin {{name}} {{args}}

run-dump *args:
    cargo run --bin {{name}}-dump {{args}}

run-fake-item *args:
    cargo run --features testkit --example publish_item -- {{args}}

install:
    install -Dm0755 {{bin-src}} {{bin-dst}}
    install -Dm0644 resources/{{appid}}.desktop {{desktop-dst}}
    install -Dm0644 resources/{{appid}}.metainfo.xml {{metainfo-dst}}
    install -Dm0644 resources/{{appid}}.svg {{icon-dst}}
    install -Dm0644 resources/{{appid}}-symbolic.svg {{symbolic-icon-dst}}

uninstall:
    rm -f {{bin-dst}} {{desktop-dst}} {{metainfo-dst}} {{icon-dst}} {{symbolic-icon-dst}}

install-user:
    install -Dm0755 {{bin-src}} {{user-bin-dst}}
    install -Dm0644 resources/{{appid}}.metainfo.xml {{user-metainfo-dst}}
    install -Dm0644 resources/{{appid}}.svg {{user-icon-dst}}
    install -Dm0644 resources/{{appid}}-symbolic.svg {{user-symbolic-icon-dst}}
    mkdir -p "$(dirname {{user-desktop-dst}})"
    sed 's|^Exec=.*|Exec={{user-bin-dst}}|' resources/{{appid}}.desktop > {{user-desktop-dst}}
    chmod 0644 {{user-desktop-dst}}
    @echo "Installed. Add 'Status Hub' in Settings -> Desktop -> Panel -> Applets."

uninstall-user:
    rm -f {{user-bin-dst}} {{user-desktop-dst}} {{user-metainfo-dst}} {{user-icon-dst}} {{user-symbolic-icon-dst}}

flatpak-sources:
    flatpak run --filesystem="$(pwd)" --share=network \
        --command=flatpak-cargo-generator org.flatpak.Builder \
        "$(pwd)/Cargo.lock" -o "$(pwd)/{{flatpak-dir}}/cargo-sources.json"

flatpak-build:
    flatpak-builder --user --install --force-clean --ccache \
        {{flatpak-build-dir}} {{manifest}}

flatpak-build-local:
    #!/usr/bin/env bash
    set -euo pipefail
    local_manifest="{{flatpak-dir}}/local-test.json"
    trap 'rm -f "${local_manifest}"' EXIT
    python3 -c 'import json,sys; m=json.load(open(sys.argv[1])); m["modules"][0]["sources"][0]={"type":"dir","path":"../..","skip":["target","build",".flatpak-builder",".git","flatpak"]}; json.dump(m,open(sys.argv[2],"w"),indent=2)' \
        "{{manifest}}" "${local_manifest}"
    flatpak-builder --user --install --force-clean --ccache \
        {{flatpak-build-dir}}-local "${local_manifest}"

flatpak-run *args:
    flatpak run {{appid}} {{args}}

flatpak-dump:
    flatpak run --command={{name}}-dump {{appid}}

flatpak-uninstall:
    flatpak uninstall --user -y {{appid}}
