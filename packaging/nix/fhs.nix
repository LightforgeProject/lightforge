{
  symlinkJoin,
  makeWrapper,
  steam,
  lightforge-unwrapped,
}:

# NixOS has no /lib/ld-linux.so.2 with or without hardware.graphics.enable32Bit,
# and Proton's binaries need it, so setup would stop at the 32-bit check.
# steam-run-free is steam-run without the unfree Steam package.
let
  # Its FreeType is 64-bit only; without a 32-bit one Battle.net has no text.
  steam-run-free = (steam.override { extraLibraries = pkgs: [ pkgs.freetype ]; }).run-free;
in
symlinkJoin {
  name = "${lightforge-unwrapped.pname}-${lightforge-unwrapped.version}";
  paths = [ lightforge-unwrapped ];
  nativeBuildInputs = [ makeWrapper ];

  postBuild = ''
    # Whichever binaries the package built.
    for exe in ${lightforge-unwrapped}/bin/*; do
      bin=''${exe##*/}
      rm $out/bin/$bin
      makeWrapper ${steam-run-free}/bin/steam-run $out/bin/$bin \
        --add-flags ${lightforge-unwrapped}/bin/$bin \
        --set LIGHTFORGE_WRAPPER $out/bin/$bin
    done
  '';

  inherit (lightforge-unwrapped) version meta;
}
