# Cobblestone (Rust)

CLI tool to scrobble Rockbox `playback.log` entries to Last.fm or Libre.fm.
Cobblestone does not depend on the Rockbox Last.fm plugin.
Cobblestone currently supports Rockbox 4.0.0 only due to tagcache database
format specifics.

## Rockbox playback logging

Playback logging must be enabled for `playback.log` to be populated. In Rockbox:

1. Open **Settings**.
2. Go to **Playback Settings**.
3. Enable **Logging**.

The log is written to `<rockbox-dir>/playback.log`.

## Build

```bash
cargo build
```

Release build:

```bash
cargo build --release
```

## Run

Basic usage:

```bash
cargo run -- scrobble --rockbox-dir /path/to/.rockbox
```

To see the installed binary name and top-level help:

```bash
cargo run -- --help
```

### CLI options

Top-level commands:

- `service set-keys`: set Last.fm API key/secret (Libre.fm uses `cobblestone/cobblestone`).
- `account add|remove|list`: manage accounts.
- `scrobble`: parse and scrobble `playback.log`.

`service set-keys`:

```bash
cobblestone service set-keys <service> --api-key <key> --api-secret <secret> [--config-path <path>]
```

Notes:
- Only `lastfm` is accepted for `<service>`.

`account add`:

```bash
cobblestone account add <service> --username <name> [--password <pwd>] [--config-path <path>]
```

`account remove`:

```bash
cobblestone account remove <service> --username <name> [--config-path <path>]
```

`account list`:

```bash
cobblestone account list [--service <service>] [--config-path <path>]
```

`scrobble`:

```bash
cobblestone scrobble \
  [--rockbox-dir <path>] \
  [--playback-log <path>] \
  [--service <service>] \
  [--username <name>] \
  [--config-path <path>] \
  [--no-truncate] \
  [--dry-run] \
  [--debug-response]
```

Options:
- `--rockbox-dir`: path to the `.rockbox` directory (default: `.rockbox`)
- `--playback-log`: explicit path to `playback.log` (default: `<rockbox-dir>/playback.log`)
- `--service`: limit to one service (`lastfm` or `librefm`)
- `--username`: limit to one username
- `--config-path`: config file location (default: `~/.config/cobblestone/config.json`)
- `--no-truncate`: keep `playback.log` after scrobbling
- `--dry-run`: parse and report without scrobbling
- `--debug-response`: print raw scrobble API responses

### Config

Config defaults to `~/.config/cobblestone/config.json`.
Libre.fm does not require API keys; Last.fm does and must be set via
`service set-keys`.

### Getting Last.fm API keys

1. Sign in to your Last.fm account.
2. Visit https://www.last.fm/api/account/create to create a new API application.
3. Leave the callback URL empty for this CLI tool.
4. Copy the provided API key and shared secret.
5. Configure them with:

```bash
cobblestone service set-keys lastfm --api-key <key> --api-secret <secret>
```

## Contributing

1. Fork the repository.
2. Create a feature branch.
3. Open a pull request with a clear description of the changes and any tests run.
