<p align="center">
  <img src="assets/icon.png" width="96" alt="Skiff">
</p>

<h1 align="center">Skiff</h1>

<p align="center">
  <b>A fast FTP / FTPS / SFTP client in pure Rust with a modern dark UI</b><br>
  Dual panes, a site manager and a transfer queue: the FileZilla workflow without the clutter.
</p>

<p align="center">
  <a href="https://github.com/just4life/skiff/releases/latest"><b>⬇ Download</b></a> ·
  <a href="README.md">Русский</a>
</p>

![Skiff](docs/screenshot-main.png)

> The UI is currently in Russian.

## Features

- **Protocols:** FTP, FTPS (explicit `AUTH TLS` and implicit, port 990), SFTP over SSH.
- **SFTP auth:** password, keyboard-interactive, OpenSSH/PEM keys (ed25519, ECDSA, RSA), including encrypted keys.
- **Trust on first use:** unknown SSH host keys and unverified TLS certificates are shown with their SHA-256 fingerprint. A changed key is flagged as a possible attack.
- **Site manager:** groups, search, duplicate, autosave, and **import of FileZilla's `sitemanager.xml`**.
- **Quickconnect** that remembers its fields and accepts URLs like `sftp://user@host:2222/path`.
- **Dual panes:** sortable columns, natural sort, filter, history, multi-select, context menu, hotkeys.
- **Transfers:** drag and drop between panes (and from the OS file manager), recursive folders, a queue with progress and speed, pause, cancel, retry, and **N parallel workers**, each with its own connection.
- **File exists:** ask / overwrite / if newer / resume (REST/APPE, SFTP seek) / rename / skip, with "apply to all".
- **Robust:** keep-alive, automatic reconnect, PASV NAT workaround, EPSV for IPv6, MLSD with LIST fallback, UTF-8.

## Command line

Connect right at startup, like FileZilla:

```bash
skiff sftp://root:password@192.168.1.10
skiff --site "Group/Site name" --local ~/projects
```

`-c/--site` connects to a saved site (`Name`, `Group/Name` or FileZilla's `0/Group/Name`),
`-a/--local` opens a local folder, `-h/--help` prints help. Escape special characters in the
password URL-style (`@` → `%40`).

## Build

Rust 1.88+ (edition 2024):

```bash
cargo build --release
./target/release/skiff
```

Cross-compiling for Windows from Linux:

```bash
rustup target add x86_64-pc-windows-gnu && sudo apt install mingw-w64
cargo build --release --target x86_64-pc-windows-gnu
```

Rendering uses OpenGL, so it also works in VMs, over RDP and on old GPUs.

## License

GPL-3.0-or-later, compatible with FileZilla (GPL-2.0-or-later). Inter font: SIL Open Font License 1.1.
