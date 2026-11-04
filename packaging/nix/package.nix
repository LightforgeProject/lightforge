{
  lib,
  rustPlatform,
  pkg-config,
  udev,
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
  pname = "lightforge";
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

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ udev ];

  postInstall = ''
    install -Dm644 packaging/pro.tuxcore.Lightforge.desktop -t $out/share/applications
    install -Dm644 packaging/pro.tuxcore.Lightforge.metainfo.xml -t $out/share/metainfo
    cp -r packaging/icons $out/share/icons
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath runtimeLibs} $out/bin/lightforge-gui
  '';

  passthru = { inherit runtimeLibs; };

  meta = {
    description = "Install, update and play World of Warcraft on Linux, and a home for all your other Battle.net games";
    homepage = cargoToml.workspace.package.homepage;
    license = lib.licenses.gpl3Plus;
    mainProgram = "lightforge-gui";
    platforms = [ "x86_64-linux" ];
  };
}
