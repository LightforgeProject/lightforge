# Building the Flatpak

The Flatpak manifest is [packaging/flatpak/pro.tuxcore.Lightforge.yml](packaging/flatpak/pro.tuxcore.Lightforge.yml). It builds both Lightforge binaries from the checked-out source and reuses the project's desktop entry, AppStream metadata and icons.

## Requirements

Install Flatpak, `flatpak-builder`, and the Freedesktop 26.08 runtime and SDK. On Fedora, for example:

```sh
sudo dnf install flatpak flatpak-builder
flatpak install flathub \
  org.freedesktop.Platform//26.08 \
  org.freedesktop.Sdk//26.08 \
  org.freedesktop.Sdk.Extension.rust-stable//26.08
```

The Rust extension is used by the manifest to provide Cargo inside the SDK. The first build downloads the crates named by `Cargo.lock`, so the build needs network access.

The manifest also requests Freedesktop's 32-bit compatibility and graphics extensions. They are required because Battle.net's installer is a 32-bit Windows program; they are downloaded as part of the Flatpak installation.

## Build and run locally

From the repository root:

```sh
flatpak-builder --force-clean --user --install-deps-from=flathub --install \
  flatpak-build/build-dir \
  packaging/flatpak/pro.tuxcore.Lightforge.yml
flatpak run pro.tuxcore.Lightforge
```

The manifest uses the working tree as its source, so changes must be saved before running `flatpak-builder`. The generated build directory lives under `flatpak-build/`, which is ignored by Git. Remove that directory, or keep `--force-clean`, when switching branches or troubleshooting a stale build.

## Permissions and game files

The manifest grants read/write access to `~/Games/Blizzard` by default, which is the standard Lightforge library path. It intentionally grants no access to other directories or mounted drives. Add every additional directory that Lightforge should be allowed to use yourself. This keeps the Flatpak from exposing your whole home directory or every mounted drive by default.

Lightforge needs write access to the directories containing its settings, Proton builds, Wine prefixes, and game libraries. File and folder selection still goes through the desktop portal, but a selected directory must also be allowed by the Flatpak permission system.

### Using Flatseal

1. Install and open **Flatseal**.
2. Select **Lightforge** (`pro.tuxcore.Lightforge`).
3. Under **Filesystem**, add the directories Lightforge should use in **Other files**.
4. Give each entry read/write access when Lightforge needs to download or update files.
5. Restart Lightforge after changing permissions.

Do not add `~/.var/app/pro.tuxcore.Lightforge` as a permission. Flatpak automatically gives the application access to its private app-data directory there. The native-install path `~/.local/share/lightforge` is not the path used by the Flatpak.

For example, add `/home/you/Games/WorldOfWarcraft` for a game library outside the default path or the mount point containing it. Use the actual paths on your system; do not copy this example path literally.

### Using the command line

Repeat `--filesystem` for each directory. The `:rw` suffix grants read/write access:

```sh
flatpak override --user \
  --filesystem=/home/you/Games/WorldOfWarcraft:rw \
  pro.tuxcore.Lightforge
```

To allow all of your home directory, use this broader permission instead:

```sh
flatpak override --user --filesystem=home:rw pro.tuxcore.Lightforge
```

To inspect the current permissions:

```sh
flatpak override --user --show pro.tuxcore.Lightforge
```

Flatpak stores the application's private configuration, data, and cache below `~/.var/app/pro.tuxcore.Lightforge/`. The exact subdirectories are managed through the XDG environment inside the sandbox; they do not need to be added manually.

The Flatpak does not bundle Wine, Proton, or Blizzard game data. Lightforge downloads and manages Proton itself; Battle.net or other Windows components still need the host's compatible 32-bit support.

## Updating the manifest

The application ID must remain `pro.tuxcore.Lightforge`; it matches the desktop entry, AppStream metadata, and GUI window ID. When the project version changes, update the normal project metadata in `Cargo.toml` and the release metadata as usual. The manifest intentionally does not duplicate that version.

For a distributable or Flathub build, replace the local directory source with a tagged source and generate pinned Cargo sources with `flatpak-cargo-generator.py`. The local-tree source and network-enabled Cargo build are intended for building this project from a checkout; they are not a Flathub submission recipe.
