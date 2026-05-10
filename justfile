name        := 'cosmic-toys'
prefix      := '/usr'
bin-dir     := prefix / 'bin'
app-dir     := prefix / 'share' / 'applications'
unit-dir    := prefix / 'lib' / 'systemd' / 'user'
icon-dir    := prefix / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps'
metainfo-dir:= prefix / 'share' / 'metainfo'
i18n-dir    := prefix / 'share' / name

# Default: build all three release binaries.
default: build-release

build-release:
    cargo build --release --workspace

# Build, then install everything to {{prefix}} (use sudo).
install: build-release
    install -Dm0755 target/release/cosmic-toysd       {{bin-dir}}/cosmic-toysd
    install -Dm0755 target/release/cosmic-toys        {{bin-dir}}/cosmic-toys
    install -Dm0755 target/release/cosmic-toys-applet {{bin-dir}}/cosmic-toys-applet
    install -Dm0644 gui/resources/com.pyxyll.CosmicToys.desktop \
                    {{app-dir}}/com.pyxyll.CosmicToys.desktop
    install -Dm0644 applet/resources/com.pyxyll.CosmicToysApplet.desktop \
                    {{app-dir}}/com.pyxyll.CosmicToysApplet.desktop
    install -Dm0644 gui/resources/com.pyxyll.CosmicToys.svg \
                    {{icon-dir}}/com.pyxyll.CosmicToys.svg
    install -Dm0644 gui/resources/com.pyxyll.CosmicToys.metainfo.xml \
                    {{metainfo-dir}}/com.pyxyll.CosmicToys.metainfo.xml
    install -Dm0644 dist/systemd/cosmic-toysd.service \
                    {{unit-dir}}/cosmic-toysd.service

uninstall:
    rm -f {{bin-dir}}/cosmic-toysd
    rm -f {{bin-dir}}/cosmic-toys
    rm -f {{bin-dir}}/cosmic-toys-applet
    rm -f {{app-dir}}/com.pyxyll.CosmicToys.desktop
    rm -f {{app-dir}}/com.pyxyll.CosmicToysApplet.desktop
    rm -f {{icon-dir}}/com.pyxyll.CosmicToys.svg
    rm -f {{metainfo-dir}}/com.pyxyll.CosmicToys.metainfo.xml
    rm -f {{unit-dir}}/cosmic-toysd.service

clean:
    cargo clean
