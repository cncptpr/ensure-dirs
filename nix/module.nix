{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.ensureDirs;

  # The full configuration handed to the Rust program, exactly as JSON.
  # `package` is the only key that is not configuration data (it is a
  # derivation), so it is stripped; everything else is passed verbatim.
  configFile = pkgs.writeText "ensure-dirs-config.json" (
    builtins.toJSON (removeAttrs cfg [ "package" ])
  );
in
{
  options.ensureDirs = {
    useCache = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether to keep a cached state of the last applied configuration in
        {option}`ensureDirs.cacheDir`. On later runs the current configuration
        is diffed against that state, and directories that are already known
        to be correct are skipped without any filesystem interaction.
        Disable to force a full check on every run.
      '';
    };

    cacheDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/cache/ensure-dirs";
      example = "/var/cache/my-dirs";
      description = ''
        Directory in which the state file `state.json` is kept.
        Only used when {option}`ensureDirs.useCache` is enabled.
      '';
    };

    dirs = lib.mkOption {
      type = lib.types.attrsOf (
        lib.types.submodule {
          options = {
            user = lib.mkOption {
              type = lib.types.str;
              example = "alice";
              description = "Owner the directory must have.";
            };

            group = lib.mkOption {
              type = lib.types.str;
              example = "users";
              description = "Group the directory must have.";
            };

            mode = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              example = "0750";
              description = ''
                Octal permission mode to enforce (for example "0750").
                When null, permissions are not managed.
              '';
            };
          };
        }
      );
      default = { };
      example = lib.literalExpression ''
        {
          "/srv/data" = {
            user = "alice";
            group = "users";
            mode = "0750";
          };
          "/srv/incoming" = {
            user = "bob";
            group = "upload";
          };
        }
      '';
      description = ''
        Directories to ensure. Keys are absolute directory paths; the value
        describes the owner, group and (optionally) mode the directory must
        have. Directories are created with all missing parents as needed.
        Entries removed from this option are never deleted from disk - only
        no longer tracked.
      '';
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
      description = "The ensure-dirs package to run at activation.";
    };
  };

  # No activation snippet (and no package) at all when no directories are
  # configured - there is no separate enable option by design.
  config = lib.mkIf (cfg.dirs != { }) {
    environment.systemPackages = [ cfg.package ];

    system.activationScripts.ensureDirs = {
      deps = [
        "specialfs"
        "users"
        "groups"
      ];
      text = ''
        echo "configuring ensure-dirs..."
        ${lib.getExe cfg.package} --config ${configFile}
      '';
    };
  };
}
