{
  description = "Lightforge: a native Linux launcher for World of Warcraft and Battle.net";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [ "x86_64-linux" ];
      forAllSystems = f: lib.genAttrs systems (system: f system nixpkgs.legacyPackages.${system});
    in
    {
      overlays.default = final: _prev: {
        lightforge-unwrapped = final.callPackage ./packaging/nix/package.nix { };
        lightforge = final.callPackage ./packaging/nix/fhs.nix { };
        lightforge-cli = final.callPackage ./packaging/nix/fhs.nix {
          lightforge-unwrapped = final.lightforge-unwrapped.override { withGui = false; };
        };
      };

      packages = forAllSystems (_: pkgs: rec {
        lightforge-unwrapped = pkgs.callPackage ./packaging/nix/package.nix { };
        lightforge = pkgs.callPackage ./packaging/nix/fhs.nix { inherit lightforge-unwrapped; };
        lightforge-cli = pkgs.callPackage ./packaging/nix/fhs.nix {
          lightforge-unwrapped = lightforge-unwrapped.override { withGui = false; };
        };
        default = lightforge;
      });

      devShells = forAllSystems (system: pkgs:
        let lightforge = self.packages.${system}.lightforge-unwrapped; in
        {
          default = pkgs.mkShell {
            inputsFrom = [ lightforge ];
            packages = [ pkgs.clippy pkgs.rustfmt pkgs.rust-analyzer ];
            # Not LD_LIBRARY_PATH, which Proton and the game would inherit.
            RUSTFLAGS = "-C link-arg=-Wl,-rpath,${lib.makeLibraryPath lightforge.runtimeLibs}";
          };
        });
    };
}
