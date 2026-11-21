# booker-search (`bs`)

A fast, fuzzy file finder with an interactive TUI and SSH support. Search local
and remote filesystems simultaneously, open results directly in your editor, and
browse the file tree — all from the terminal.

## Features

- **Fuzzy and consecutive matching** — standard fuzzy search or ordered-letter mode
- **Wildcard search** — use `*` and `?` for glob-style matching, auto-detected
- **Content search** — grep inside file contents locally and over SSH
- **SSH locations** — search remote filesystems transparently via SSH
- **File browser** — navigate directories from any search result
- **Index + live modes** — fast index search, live scan, or both together
- **Smart location filtering** — `^L` filters in-place when results are ready, restarts only when needed
- **CLI mode** — pipe results to other tools: `bs '*.rs' | xargs grep TODO`
- **Man page** — `man bs`

## Installation

### NixOS (traditional configuration, no flakes)

Get the sha256 hash for the version you want:

```bash
nix-prefetch-url --unpack \
  https://github.com/ryanbooker1/booker-search/archive/main.tar.gz
```

Add to your `/etc/nixos/configuration.nix`:

```nix
{ config, pkgs, ... }:

let
  booker-search = pkgs.rustPlatform.buildRustPackage {
    pname = "booker-search";
    version = "0.1.0";

    src = pkgs.fetchFromGitHub {
      owner  = "ryanbooker1";
      repo   = "booker-search";
      rev    = "main";
      sha256 = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    };

    cargoLock.lockFile = ./Cargo.lock;

    nativeBuildInputs = with pkgs; [ pkg-config ];
    buildInputs       = with pkgs; [ openssl libssh2 zlib ];

    PKG_CONFIG_PATH = pkgs.lib.makeSearchPathOutput "dev" "lib/pkgconfig"
      (with pkgs; [ openssl libssh2 zlib ]);

    postInstall = ''
      install -Dm644 man/bs.1 $out/share/man/man1/bs.1
    '';
  };
in
{
  environment.systemPackages = [ booker-search ];
}
```

Then rebuild:

```bash
sudo nixos-rebuild switch
```

See [`configuration.nix.example`](configuration.nix.example) for a full annotated example including update instructions.

### Any Linux (via Nix package manager)

Install [Nix](https://install.determinate.systems) on any distro, then:

```bash
nix profile install github:ryanbooker1/booker-search
```

### Arch / Manjaro / Omarchy / Ubuntu (via Cargo)

Install system dependencies first:

```bash
# Arch / Manjaro / Omarchy
sudo pacman -S pkg-config openssl libssh2

# Ubuntu / Debian
sudo apt install pkg-config libssl-dev libssh2-1-dev
```

Then install:

```bash
cargo install --git https://github.com/ryanbooker1/booker-search --bin bs
```

## Usage

### Interactive TUI

```bash
bs
```

Type to search. Results appear as you type from the index; live searches
fill in new files after a short delay.

### CLI (non-interactive)

```bash
bs <query> [options]
```

Prints matching paths to stdout — no TUI, works in scripts and pipes.

```bash
bs main                        # find files matching "main"
bs '*.rs'                      # wildcard: all Rust source files
bs -m live main                # live scan only (bypass index)
bs -f TODO                     # search file contents for "TODO"
bs -c README                   # case-sensitive match
bs -t dirs src                 # directories only
bs -l work query               # restrict to the "work" location
bs -n 20 '*.toml'              # limit to 20 results
bs -0 '*.rs' | xargs -0 wc -l  # NUL-separated output for xargs
```

### CLI options

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--target` | `-t` | `files` | What to search: `files`, `dirs`, `all`, `content` |
| `--mode` | `-m` | `both` | Search mode: `index`, `live`, `both` |
| `--content` | `-f` | off | Also grep file contents |
| `--case-sensitive` | `-c` | off | Case-sensitive matching |
| `--consecutive` | `-z` | off | Letters must appear in order, no gaps |
| `--location` | `-l` | all | Filter to one location by name or ID |
| `--limit` | `-n` | `100` | Maximum results to print |
| `--null` | `-0` | off | NUL-separated output (for `xargs -0`) |

CLI flags use built-in defaults and do not read or modify your saved settings.

## Keyboard shortcuts

| Key | Action |
|-----|--------|
| `↑ / ↓` | Move through results |
| `Enter` | Open in editor |
| `Esc` | Clear query, cancel searches |
| `^F` | Toggle content search |
| `^T` | Cycle search target (files → dirs → all → content) |
| `^S` | Cycle search mode (index → live → both) |
| `^Z` | Toggle fuzzy / consecutive matching |
| `^A` | Toggle case-sensitive matching |
| `^L` | Cycle location filter |
| `^N` | File info popup |
| `^C` / `→` | Open file browser |
| `^R` | Rebuild index |
| `Tab` | Open settings popup |
| `^Q` | Quit |

Full reference: `man bs` or press `Tab` → Keys in the TUI.

## Configuration

Settings and locations are managed from the TUI (`Tab` to open settings).
Config is stored in `~/.config/booker-search/config.toml`.

SSH passwords are stored separately — either encrypted
(`~/.config/booker-search/credentials.enc`, requires a master password on
startup) or in a plain file with restricted permissions
(`~/.config/booker-search/credentials.json`). Neither is ever written to disk
in a world-readable state. No credentials are stored in this repository.

SSH host keys are verified using trust-on-first-use (TOFU) and stored in
`~/.config/booker-search/known_hosts`.

## Building from source

Requires Rust and the system libraries listed in the Cargo install section above.

```bash
git clone https://github.com/ryanbooker1/booker-search
cd booker-search

# With Nix (recommended — handles all dependencies)
nix develop --command cargo build --release

# Without Nix (system deps must be installed manually)
cargo build --release
```

The binary is at `target/release/bs`. Copy it to anywhere on your `$PATH`.

## License

MIT
