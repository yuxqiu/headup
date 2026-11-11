{
  description = "headup: head-posture reminders from earbud head tracking";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      # Commands the detector runs for cues (pw-play, notify-send).
      runtimeDeps = pkgs: [
        pkgs.pipewire
        pkgs.libnotify
      ];
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "headup";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [
            pkgs.makeWrapper
            pkgs.pkg-config
          ];
          buildInputs = [ pkgs.dbus ]; # BlueZ D-Bus API (device auto-detection)
          postInstall = ''
            wrapProgram $out/bin/headup \
              --prefix PATH : ${pkgs.lib.makeBinPath (runtimeDeps pkgs)}
          '';
          meta.mainProgram = "headup";
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.python3 # tools/*.py (stdlib only)
            pkgs.espeak-ng # spoken prompts in tools/guided.py and tools/wear_test.py
          ]
          ++ runtimeDeps pkgs;
        };
      });

      homeManagerModules.default = import ./nix/hm-module.nix self;

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
