{
  description = "ensure-dirs: idempotent directory owner/group/mode enforcer for NixOS activation";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f system nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (
        system: pkgs:
        let
          pkg = pkgs.callPackage ./nix/package.nix { };
        in
        {
          default = pkg;
          ensure-dirs = pkg;
        }
      );

      devShells = forAllSystems (
        system: pkgs: {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              rustc
              clippy
              rustfmt
              rust-analyzer
              nixfmt
            ];
            RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
          };
        }
      );

      formatter = forAllSystems (system: pkgs: pkgs.nixfmt);

      nixosModules = {
        default = import ./nix/module.nix;
        ensureDirs = import ./nix/module.nix;
      };

      checks = forAllSystems (
        system: pkgs:
        let
          pkg = self.packages.${system}.default;

          evalConfig =
            modules:
            (lib.nixosSystem {
              inherit system;
              modules = [
                self.nixosModules.ensureDirs
                { system.stateVersion = "25.10"; }
              ]
              ++ modules;
            }).config;

          withDirs = evalConfig [
            {
              ensureDirs = {
                useCache = true;
                cacheDir = "/var/cache/ensure-dirs";
                dirs = {
                  "/srv/data" = {
                    user = "root";
                    group = "root";
                    mode = "0750";
                  };
                  "/srv/misc" = {
                    user = "nobody";
                    group = "nogroup";
                  };
                };
              };
            }
          ];

          withoutDirs = evalConfig [ ];
        in
        {
          # Builds the crate and runs `cargo test` in the sandbox.
          package = pkg;

          # End-to-end: force full dependency-resolved activation script
          # (catches broken deps/ordering), then extract the JSON config
          # file the module generated and have the real binary validate it.
          module-eval =
            let
              full = withDirs.system.activationScripts.script;
              snippet = withDirs.system.activationScripts.ensureDirs.text;
            in
            assert lib.hasInfix "#### Activation script snippet ensureDirs:" full;
            assert lib.hasInfix "--config /nix/store/" snippet;
            pkgs.runCommand "check-ensure-dirs-module-eval"
              {
                inherit snippet;
                inherit (withDirs.system.activationScripts) script;
                passAsFile = [ "script" ];
              }
              ''
                # our snippet must exist and run after the mount and user
                # snippets ("groups" is an empty ordering hook and emits no
                # header, but is still resolved by the closure - enforced by
                # evaluating `activationScripts.script` above)
                ours=$(grep -n '#### Activation script snippet ensureDirs:' "$scriptPath" | cut -d: -f1 | head -n1)
                test -n "$ours"
                for dep in specialfs users; do
                  at=$(grep -n "#### Activation script snippet $dep:" "$scriptPath" | cut -d: -f1 | head -n1)
                  test -n "$at"
                  test "$at" -lt "$ours"
                done
                config=$(printf '%s' "$snippet" | grep -oE '/nix/store/[^ ]+ensure-dirs-config\.json' | head -n1)
                test -n "$config"
                ${pkg}/bin/ensure-dirs --check-config --config "$config"
                touch $out
              '';

          # An empty `dirs` attribute set must disable the module entirely.
          module-empty =
            assert !(withoutDirs.system.activationScripts ? ensureDirs);
            assert !(lib.elem pkg withoutDirs.environment.systemPackages);
            pkgs.runCommand "check-ensure-dirs-module-empty" { } ''
              touch $out
            '';
        }
      );
    };
}
