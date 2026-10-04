# ensure-dirs

Ensures that a configured set of directories exists with the correct owner,
group and (optionally) mode. Designed to run as root from a NixOS activation
script, with an opt-out state cache so that repeated runs need almost no
filesystem interaction.

## Configuration (NixOS)

```nix
{
  imports = [ inputs.ensure-dirs.nixosModules.default ];

  ensureDirs = {
    useCache = true;
    cacheDir = "/var/cache/ensure-dirs";
    dirs = {
      "/srv/data" = {
        user = "alice";
        group = "users";
        mode = "0750";
      };
      "/srv/incoming" = {
        user = "bob";
        group = "upload";
        # mode is optional; null (the default) means "don't manage permissions"
      };
    };
  };
}
```

The whole `ensureDirs` attribute set is serialized with `builtins.toJSON`
and handed to the Rust program. When `dirs` is empty the activation snippet
is not registered at all (there is no separate `enable` option).

## Behaviour

| Situation | Action |
|---|---|
| Directory missing | `mkdir -p`, then chown/chmod as configured |
| Exists with wrong owner/mode | chown/chmod (only the mismatching call) |
| Unchanged since last cached run | **skipped entirely** (no syscalls) |
| Removed from configuration | left on disk, dropped from the cache state |
| Any failure (unknown user, path is a file, ...) | reported, exit code 1, activation fails |

The state file lives at `<cacheDir>/state.json` (mode 0600, written
atomically). With `useCache = false` (or `--force`) every directory is
stat-checked on every run.

## Development

```sh
nix develop          # Rust toolchain
cargo test           # unit tests
cargo run -- --check-config --config some.json

nix build            # build the package (also runs cargo test)
nix run . -- --help
nix flake check       # package + NixOS module evaluation checks
```

## CLI

```
ensure-dirs --config <file> [--check-config] [--force] [--dry-run] [--verbose]
```
