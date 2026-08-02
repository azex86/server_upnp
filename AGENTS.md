# server_upnp

Rust binary crate: minimal UPnP/DLNA MediaServer exposing one folder tree, no indexing.
Usage: `server_upnp <port> <folder> [display-name]`. Modules: `main.rs` (CLI/state), `ssdp.rs` (discovery),
`web.rs` (axum routes), `soap.rs` (Browse/DIDL-Lite), `desc.rs` (UPnP XML descriptions).

## Commands

```bash
cargo build              # debug build
cargo build --release    # release build
cargo run                # run the binary
cargo check              # type-check (fast)
cargo test               # run tests
cargo clippy             # lint
cargo fmt                # format code
```

Dependencies: axum, tokio, tower-http (Range-aware file serving), socket2 (SSDP multicast),
percent-encoding, mime_guess, httpdate. No tests or CI. Rust edition 2021, no pinned toolchain.
