# The ensure-dirs package. Used both by `nix build` and as the default for
# the `ensureDirs.package` module option.
{
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage {
  pname = "ensure-dirs";
  version = "0.1.0";

  # Only the crate sources participate in the build: editing flake.nix or
  # the NixOS module must not rebuild (and re-fetch) the package.
  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: type:
      let
        base = baseNameOf (toString path);
      in
      base != "target"
      && base != "result"
      && base != "result-bin"
      && base != "flake.nix"
      && base != "flake.lock"
      && base != "nix"
      && !(lib.hasSuffix ".qcow2" base);
  };

  cargoLock.lockFile = ../Cargo.lock;

  meta = {
    description = "Ensure directories exist with the configured owner, group and mode";
    mainProgram = "ensure-dirs";
    license = lib.licenses.mit;
  };
}
