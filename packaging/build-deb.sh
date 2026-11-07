#!/usr/bin/env bash
# Copyright 2026 Google LLC
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     https://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
#
# Build a native Debian package for sommelier-rs-r.
#
# Run this ON the Debian / Crostini machine that will install the package.
# Runtime shared-library dependencies are resolved from the freshly built
# binary with dpkg-shlibdeps, so the .deb matches the distro it was built on.
#
#   bash packaging/build-deb.sh
#   OUT_DIR=/tmp PKG_VERSION=0.3.4-r bash packaging/build-deb.sh
#
# Requires: cargo/rustc, dpkg-deb, dpkg-shlibdeps, strip, coreutils.
# The build dependencies of the Rust crates are the ones listed in README.md
# ("构建"): libxkbcommon-dev libgbm-dev libdrm-dev libexpat1-dev.

set -euo pipefail

die() { echo "build-deb: $*" >&2; exit 1; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# rustup installs cargo in ~/.cargo/bin; non-login shells (and sudo) often
# do not have it on PATH.
if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env" 2>/dev/null || true
fi

# sommelier/Cargo.toml is a historical CRLF file, so the pattern must tolerate
# a trailing \r instead of anchoring on `"` immediately before end-of-line.
VERSION="${PKG_VERSION:-$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$ROOT/sommelier/Cargo.toml" | head -n 1)}"
ARCH="${PKG_ARCH:-$(dpkg --print-architecture)}"
OUT_DIR="${OUT_DIR:-$ROOT/dist}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BUILD_DIR="$CARGO_TARGET_DIR/deb-build"
STAGE="$BUILD_DIR/sommelier-rs-r_${VERSION}_${ARCH}"
DEB="$OUT_DIR/sommelier-rs-r_${VERSION}_${ARCH}.deb"

[ -n "$VERSION" ] || die "cannot read version from sommelier/Cargo.toml"
dpkg --validate-version "$VERSION" >/dev/null 2>&1 || die "invalid Debian version: $VERSION"

for tool in cargo dpkg-deb dpkg-shlibdeps strip install; do
    command -v "$tool" >/dev/null 2>&1 || die "missing tool: $tool"
done

echo "==> building release binary (sommelier-rs-r $VERSION / $ARCH)"
echo "    source:  $ROOT"
echo "    target:  $CARGO_TARGET_DIR"
(cd "$ROOT" && CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo build --release --workspace --locked)
BIN="$CARGO_TARGET_DIR/release/sommelier-rs-r"
[ -x "$BIN" ] || die "release binary not found at $BIN"

echo "==> staging $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE/DEBIAN" \
         "$STAGE/usr/bin" \
         "$STAGE/usr/lib/systemd/user" \
         "$STAGE/usr/share/doc/sommelier-rs-r"

install -m 0755 "$BIN" "$STAGE/usr/bin/sommelier-rs-r"
strip --strip-unneeded "$STAGE/usr/bin/sommelier-rs-r" || true

# Both the Wayland-only and the X11 systemd user templates are shipped.
for unit in "$ROOT"/packaging/systemd/*.service; do
    [ -f "$unit" ] || continue
    install -m 0644 "$unit" "$STAGE/usr/lib/systemd/user/$(basename "$unit")"
done

install -m 0644 "$ROOT/README.md" "$STAGE/usr/share/doc/sommelier-rs-r/README.md"
install -m 0644 "$ROOT/LICENSE" "$STAGE/usr/share/doc/sommelier-rs-r/copyright"
for doc in "$ROOT"/docs/*.md; do
    [ -f "$doc" ] || continue
    install -m 0644 "$doc" "$STAGE/usr/share/doc/sommelier-rs-r/$(basename "$doc")"
done

# dpkg-shlibdeps expects a debian/control in the cwd; the temporary one only
# exists so it can resolve ${shlibs:Depends} for the real binary.
mkdir -p "$BUILD_DIR/debian"
cat > "$BUILD_DIR/debian/control" <<EOF
Source: sommelier-rs-r
Section: utils
Priority: optional
Maintainer: sommelier-rs-r contributors <noreply@example.com>
Standards-Version: 4.7.0

Package: sommelier-rs-r
Architecture: $ARCH
Depends: \${shlibs:Depends}
EOF

DEPS="$(cd "$BUILD_DIR" && dpkg-shlibdeps -O "$STAGE/usr/bin/sommelier-rs-r" 2>/dev/null \
        | sed -n 's/^shlibs:Depends=//p' | head -n 1 || true)"
if [ -z "$DEPS" ]; then
    echo "==> dpkg-shlibdeps produced no substitution; using the known Crostini dependency set"
    DEPS="libc6, libgcc-s1, libxkbcommon0, libgbm1, libdrm2, libexpat1"
fi
echo "==> Depends: $DEPS"

cat > "$STAGE/DEBIAN/control" <<EOF
Package: sommelier-rs-r
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Installed-Size: $(du -sk "$STAGE/usr" | cut -f 1)
Maintainer: sommelier-rs-r contributors <noreply@example.com>
Depends: $DEPS
Suggests: xwayland
Homepage: https://github.com/google/sommelier-rs
Description: X11-capable Wayland proxy for ChromeOS Crostini (Rust)
 A Rust implementation of the sommelier Wayland proxy used inside Crostini
 Linux containers. It forwards Wayland globals from the ChromeOS host over
 /dev/wl0, and can launch Xwayland as a window manager for X11 applications.
 .
 It installs only /usr/bin/sommelier-rs-r plus two disabled-by-default
 systemd user templates. The upstream C++ sommelier, its socket names
 (wayland-0/wayland-1) and its X displays (:0/:1) are left untouched.
 .
 Quick start: sommelier-rs-r --socket=wayland-wb -- /path/to/app
 Full guide: /usr/share/doc/sommelier-rs-r/CROSTINI_INSTALL.md
EOF

cat > "$STAGE/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
case "$1" in
  configure)
    echo "sommelier-rs-r: binary installed at /usr/bin/sommelier-rs-r"
    echo "sommelier-rs-r: quick start -- sommelier-rs-r --socket=wayland-wb -- /path/to/app"
    echo "sommelier-rs-r: guide       -- /usr/share/doc/sommelier-rs-r/CROSTINI_INSTALL.md"
    echo "sommelier-rs-r: no service enabled; the live sommelier is untouched"
    ;;
esac
exit 0
EOF
chmod 0755 "$STAGE/DEBIAN/postinst"

# dpkg-deb records directory modes verbatim; normalise them to the Debian
# standard 0755 instead of whatever umask produced during staging.
find "$STAGE" -type d -exec chmod 0755 {} +

echo "==> building $DEB"
mkdir -p "$OUT_DIR"
dpkg-deb --build --root-owner-group -Zxz "$STAGE" "$DEB"
ls -lh "$DEB"
echo "==> control"
dpkg-deb -I "$DEB" | sed -n '1,22p'
echo "==> done"
