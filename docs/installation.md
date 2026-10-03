# Installing OpenSuperCAD

OpenSuperCAD is released continuously: every change merged to `main` is
published as a [GitHub release](https://github.com/1ARdotNO/OpenSuperCAD/releases)
with packages for Linux (Debian/Ubuntu and Arch) and macOS.

You need three things:

1. **OpenSuperCAD** itself (this page).
2. **OpenSCAD**, the geometry engine OpenSuperCAD drives for rendering,
   exporting and taking snapshots.
3. **An ACP agent** to design with, such as Claude Code, Gemini CLI, Codex,
   Goose or OpenCode. See [agents.md](agents.md).

## Debian / Ubuntu

```sh
# pick the .deb for your architecture (amd64 or arm64) from the latest release
curl -LO https://github.com/1ARdotNO/OpenSuperCAD/releases/latest/download/opensupercad_<version>-1_amd64.deb
sudo apt install ./opensupercad_<version>-1_amd64.deb
```

The package recommends `openscad` and `mesa-vulkan-drivers`, which `apt`
installs by default. GPUI renders with Vulkan, so a working Vulkan driver is
required: Mesa for Intel/AMD, or the proprietary NVIDIA driver.

## Arch Linux

From the AUR, if you use an AUR helper:

```sh
yay -S opensupercad-bin      # prebuilt; or `opensupercad` to build from source
```

Each release also ships a prebuilt package:

```sh
curl -LO https://github.com/1ARdotNO/OpenSuperCAD/releases/latest/download/opensupercad-<version>-1-x86_64.pkg.tar.zst
sudo pacman -U opensupercad-<version>-1-x86_64.pkg.tar.zst
sudo pacman -S --needed openscad
```

To build from source instead, use [`packaging/arch/PKGBUILD`](../packaging/arch/PKGBUILD)
(also suitable for the AUR):

```sh
git clone https://github.com/1ARdotNO/OpenSuperCAD && cd OpenSuperCAD/packaging/arch
makepkg -si
```

## macOS

With Homebrew:

```sh
brew install --cask 1ardotno/tap/opensupercad
# or, from a downloaded release asset:
brew install --cask ./opensupercad.rb
```

Or download `OpenSuperCAD-<version>-macos-universal.dmg` from the latest release.
It is a universal app for Apple silicon and Intel. Open it and drag
**OpenSuperCAD** to *Applications*.

Notarized builds open normally. If a build is only ad-hoc signed (when the
project's signing credentials aren't configured), right-click the app the
first time and choose **Open**, or run:

```sh
xattr -dr com.apple.quarantine /Applications/OpenSuperCAD.app
```

Install OpenSCAD with `brew install --cask openscad`, or from
<https://openscad.org/downloads.html>. OpenSuperCAD looks for it on `PATH`
and in `/Applications/OpenSCAD.app`.

## Generic Linux tarball

```sh
tar xzf opensupercad-<version>-x86_64-linux.tar.gz
sudo install -m755 opensupercad-<version>-x86_64-linux/opensupercad{,-mcp} /usr/local/bin/
```

## Installing OpenSCAD

| Platform | Command |
| --- | --- |
| Debian / Ubuntu | `sudo apt install openscad` |
| Arch | `sudo pacman -S openscad` |
| macOS | `brew install --cask openscad` |
| Anywhere | AppImage / nightly from <https://openscad.org/downloads.html> |

Any release from 2021.01 onwards works. Recent nightlies with the Manifold
backend render much faster: set `"openscad_backend": "manifold"` in the
project settings (see [usage.md](usage.md#project-settings)).

If OpenSCAD lives somewhere unusual, point OpenSuperCAD at it:

```sh
export OPENSUPERCAD_OPENSCAD=/opt/openscad/bin/openscad
```

### Headless machines

Snapshots need an OpenGL context. Without a display, for example over SSH or
in CI, install `xvfb` (`xvfb-run`). OpenSuperCAD detects it and uses it for
snapshots automatically. Override the wrapper with
`OPENSUPERCAD_PNG_WRAPPER="xvfb-run -a"`, or set it to `none`.

## Building from source

Requirements: a recent stable Rust toolchain (`rustup`), `git`, and on Linux
the GPUI system libraries:

```sh
# Debian / Ubuntu
sudo apt install pkg-config libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libvulkan-dev libx11-dev libx11-xcb-dev libxcb1-dev libxcb-shape0-dev \
  libxcb-xfixes0-dev libfontconfig-dev libfreetype-dev libasound2-dev libzstd-dev

# Arch
sudo pacman -S --needed base-devel libxkbcommon libxkbcommon-x11 wayland \
  vulkan-icd-loader fontconfig freetype2 libxcb alsa-lib zstd
```

macOS needs the Xcode command line tools (`xcode-select --install`) and, for
GPUI's Metal shaders, a full Xcode install.

```sh
git clone https://github.com/1ARdotNO/OpenSuperCAD
cd OpenSuperCAD
cargo run --release -p opensupercad            # the app
cargo build --release -p osc-mcp               # just the MCP server
```

## Verifying the installation

```sh
opensupercad --version
opensupercad doctor        # checks OpenSCAD, git, snapshots and known agents
```
