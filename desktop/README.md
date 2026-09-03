# OrgBot desktop (Tauri)

A desktop shell for OrgBot: it hosts a room in-process (the same engine and
agent-CLI providers as `orgbot serve`) on `ws://127.0.0.1:8787/ws` and shows
it in a webview. Agent-CLI bots run their real CLIs from here.

```bash
cd src-tauri && cargo run
```

Bundle a .deb / AppImage with the tauri CLI:

```bash
cd src-tauri && cargo install tauri-cli && cargo tauri build
```

## Packaging

Build the release binary, then package a .deb (no tauri-cli needed):

```bash
cd src-tauri && cargo build --release && cd ..
./packaging/make-deb.sh 0.1.0
sudo apt install ./orgbot-desktop_0.1.0_amd64.deb
```

The tauri `tauri.conf.json` also targets deb+appimage for `cargo tauri build`.
