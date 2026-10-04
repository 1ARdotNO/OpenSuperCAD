# Installing OpenSuperCAD

OpenSuperCAD is released continuously: every change merged to `main` is
published as a [GitHub release](https://github.com/1ARdotNO/OpenSuperCAD/releases)
with packages for Linux (Debian/Ubuntu and Arch), macOS and Windows.

You need three things:

1. **OpenSuperCAD** itself (this page).
2. **OpenSCAD**, the geometry engine OpenSuperCAD drives for rendering,
   exporting and taking snapshots. If it isn't installed, OpenSuperCAD offers
   to download the official build for you, verified against a pinned SHA-256
   checksum (see [Installing OpenSCAD](#installing-openscad)).
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

If OpenSCAD isn't installed, OpenSuperCAD offers to download it on first
start. You can also install it yourself with `brew install --cask openscad`
or from <https://openscad.org/downloads.html>; OpenSuperCAD finds it on
`PATH` and in `/Applications/OpenSCAD.app`.

## Windows

Download `OpenSuperCAD-<version>-windows-x86_64-setup.exe` from the
[latest release](https://github.com/1ARdotNO/OpenSuperCAD/releases/latest) and
run it. It installs for your user by default (no administrator prompt) and
adds OpenSuperCAD to the Start menu. A portable
`opensupercad-<version>-windows-x86_64.zip` is also published.

If OpenSCAD isn't installed, OpenSuperCAD offers to download it on first
start. You can also install it yourself from
[openscad.org](https://openscad.org/downloads.html) or with
`winget install OpenSCAD.OpenSCAD`; OpenSuperCAD finds it on `PATH` or in
`Program Files\OpenSCAD`, preferring the console build `openscad.com`. Agents
installed with npm (`npx`, `gemini`) work as on the other platforms, and git
comes from [Git for Windows](https://git-scm.com/download/win).

OpenSuperCAD opens without a console window. Command-line use
(`opensupercad doctor`, `update`, `openscad install`) prints to the terminal
you run it from; the prompt may reappear before the output, so press Enter
once it's done.

> Windows support is new. Please [report problems](https://github.com/1ARdotNO/OpenSuperCAD/issues/new?template=bug_report.yml).

## Generic Linux tarball

```sh
tar xzf opensupercad-<version>-x86_64-linux.tar.gz
install -Dm755 -t ~/.local/bin opensupercad-<version>-x86_64-linux/opensupercad{,-mcp}
```

Installed in a folder you own (like `~/.local/bin`), OpenSuperCAD can
[update itself](#updating). In `/usr/local/bin`, run
`sudo opensupercad update` instead.

## Updating

OpenSuperCAD checks GitHub for a new release when it starts (at most once a
day; turn it off in **Settings → Check for updates on start**, or set
`OPENSUPERCAD_NO_UPDATE_CHECK=1`). *Help → Check for Updates…* checks right
away. Nothing is downloaded until you click **Update**.

How the update is applied depends on how you installed OpenSuperCAD:

| Installed from | Update |
| --- | --- |
| Linux tarball (in a folder you can write to) | Replaced in place; click **Restart**. The previous binaries are kept as `*.old` |
| macOS `.dmg` (`/Applications`) | The new `.dmg` is downloaded to *Downloads* and opened; drag OpenSuperCAD to Applications |
| `.deb` or Arch `.pkg.tar.zst` from the release | The new package is downloaded to *Downloads* and verified. Installing it needs your password, so OpenSuperCAD shows the command (`sudo apt install …` / `sudo pacman -U …`) with a **Copy command** button |
| Homebrew | Use Homebrew; OpenSuperCAD tells you the command |
| Windows installer | The new installer is downloaded, verified and run silently; OpenSuperCAD closes and restarts when it's done |
| Windows `.zip` | OpenSuperCAD links to the new `.zip` |

Every download is checked before anything changes:

- The release must be **immutable** on GitHub: its tag and files are locked
  once published, so nobody can replace them later. OpenSuperCAD ignores
  releases that aren't.
- The download's SHA-256 must match both the release's `SHA256SUMS` and the
  digest GitHub records for the file. A file without a GitHub digest is never
  installed.
- `SHA256SUMS` itself must match its own GitHub digest.

Downloads use `curl` over HTTPS only. From a terminal:

```sh
opensupercad update --check   # only report
opensupercad update           # download, verify and install
```

## Installing OpenSCAD

### Let OpenSuperCAD download it

When no OpenSCAD is found, OpenSuperCAD offers to download one: in a dialog
on start, in **Settings → OpenSCAD**, in the command palette (*OpenSCAD:
Download*) and on the command line:

```sh
opensupercad openscad install   # download, verify and install
opensupercad openscad           # which OpenSCAD is used, and from where
```

What happens:

- Each OpenSuperCAD release pins one official OpenSCAD build per platform
  from [files.openscad.org](https://files.openscad.org/snapshots/): the
  AppImage on Linux (x86_64 and aarch64), the `.dmg` on macOS and the `.zip`
  on Windows. The pins, with sizes and SHA-256 checksums, are in
  [`openscad-pins.json`](../crates/osc-update/openscad-pins.json).
- The download goes over HTTPS only. It is used only if its size and SHA-256
  match the pin compiled into OpenSuperCAD, then it is unpacked and test-run
  before it replaces anything.
- It is installed for your user only, without admin rights, in the
  `openscad` folder of the data directory (see
  [usage.md](usage.md#data-locations)). Linux AppImages are extracted, so
  FUSE isn't needed. They use the system's OpenGL libraries, which every
  desktop has; on a server or minimal container install them first
  (`sudo apt install libegl1 libgl1 libopengl0 libgbm1`).
- The new OpenSCAD is used straight away, also by the agents' MCP server.

The download is the unmodified upstream build, fetched directly from
openscad.org. OpenSCAD is free software under the GPL; its source code is at
<https://github.com/openscad/openscad>.

### Use your own

| Platform | Command |
| --- | --- |
| Debian / Ubuntu | `sudo apt install openscad` |
| Arch | `sudo pacman -S openscad` |
| macOS | `brew install --cask openscad` |
| Windows | `winget install OpenSCAD.OpenSCAD` |
| Anywhere | AppImage / nightly from <https://openscad.org/downloads.html> |

Any release from 2021.01 onwards works. Recent builds (2024 and later) are
much better: the Manifold backend renders far faster (set
`"openscad_backend": "manifold"` in the project settings, see
[usage.md](usage.md#settings)) and `color()` shows in the preview.

OpenSuperCAD looks for OpenSCAD in this order:

1. `$OPENSUPERCAD_OPENSCAD`
2. the program you picked with **Locate…** (Settings → OpenSCAD, or
   `opensupercad openscad locate PATH`; *Find automatically* or
   `opensupercad openscad auto` forgets it)
3. the build OpenSuperCAD downloaded
4. `openscad` or `openscad-nightly` on `PATH`
5. the usual install locations: `/Applications/OpenSCAD.app`, Homebrew,
   Snap, Flatpak and `Program Files\OpenSCAD`

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
