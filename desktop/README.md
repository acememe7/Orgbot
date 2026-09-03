# GPTBots desktop (Tauri)

A desktop shell for GPTBots: it hosts a room in-process (the same engine and
agent-CLI providers as `gptbots serve`) on `ws://127.0.0.1:8787/ws` and shows
it in a webview. Agent-CLI bots run their real CLIs from here.

```bash
cd src-tauri && cargo run
```

Bundle a .deb / AppImage with the tauri CLI:

```bash
cd src-tauri && cargo install tauri-cli && cargo tauri build
```
