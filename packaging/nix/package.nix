{
  lib,
  rustPlatform,
  pkg-config,
  udev,
  zstd,
  vulkan-loader,
  libGL,
  wayland,
  libxkbcommon,
  # Fallbacks for package sets older than the top-level X11 names.
  xorg ? null,
  libx11 ? xorg.libX11,
  libxcursor ? xorg.libXcursor,
  libxi ? xorg.libXi,
  libxrandr ? xorg.libXrandr,
  # Off: only the command line, `lightforge`, without the launcher.
  withGui ? true,
}:

let
  cargoToml = lib.importTOML ../../Cargo.toml;

  # dlopened at run time, not linked, so they need to be on the rpath.
  runtimeLibs = [
    vulkan-loader
    libGL
    wayland
    libxkbcommon
    libx11
    libxcursor
    libxi
    libxrandr
  ];
in
rustPlatform.buildRustPackage {
  pname = if withGui then "lightforge" else "lightforge-cli";
  inherit (cargoToml.workspace.package) version;

  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.toml
      ../../Cargo.lock
      ../../crates
      # The patched iced_winit, `[patch.crates-io]` in Cargo.toml.
      ../../vendor
      ../../packaging/pro.tuxcore.Lightforge.desktop
      ../../packaging/pro.tuxcore.Lightforge.metainfo.xml
      ../../packaging/icons
      # `include_str!`d by core.
      ../../RELEASES.md
    ];
  };

  cargoLock.lockFile = ../../Cargo.lock;

  cargoBuildFlags = lib.optionals (!withGui) [ "-p" "lightforge-cli" ];
  cargoTestFlags = lib.optionals (!withGui) [ "-p" "lightforge-core" "-p" "lightforge-cli" ];

  nativeBuildInputs = [ pkg-config ];
  # zstd: Nix's Rust builds set ZSTD_SYS_USE_PKG_CONFIG, so zstd-sys links
  # the system library instead of building its bundled copy. udev: for
  # gamepads in the launcher.
  buildInputs = [ zstd ] ++ lib.optionals withGui [ udev ];

  postInstall = lib.optionalString withGui ''
    install -Dm644 packaging/pro.tuxcore.Lightforge.desktop -t $out/share/applications
    install -Dm644 packaging/pro.tuxcore.Lightforge.metainfo.xml -t $out/share/metainfo
    cp -r packaging/icons $out/share/icons
  '';

  postFixup = lib.optionalString withGui ''
    patchelf --add-rpath ${lib.makeLibraryPath runtimeLibs} $out/bin/lightforge-gui
  '';

  passthru = { inherit runtimeLibs; };

  meta = {
    description = "Install, update and play World of Warcraft on Linux, and a home for all your other Battle.net games";
    homepage = cargoToml.workspace.package.homepage;
    license = lib.licenses.gpl3Plus;
    mainProgram = if withGui then "lightforge-gui" else "lightforge";
    platforms = [ "x86_64-linux" ];
  };
}
