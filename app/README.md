# OrgBot app

Flutter UI for two execution paths:

- Android: bundled Rust FFI engine, on-device provider sign-in/API models, phone or SSH workspace tools.
- Linux: local `orgbot serve` executable and installed model CLIs.

On Linux, **Setup** shows where `codex`, `claude`, and `grok` are installed.
Authenticate in each CLI itself; OrgBot uses those local sessions. The in-app
OAuth account flow belongs to Android. In **Team**, create a bot for each CLI,
then add them to a room with a goal and a shared project folder. Their roles,
task board, handoffs, and saved room state coordinate the work. The room's
**Files** tab opens that same project folder. Finding a CLI does not verify its
account access; that is checked by the CLI when it runs.

Build the Linux app and engine before packaging. Besides Flutter and Rust,
Ubuntu builds need `clang`, `cmake`, `ninja-build`, `pkg-config`,
`libgtk-3-dev`, `libsecret-1-dev`, and `dpkg-dev`:

```sh
(cd ../core && cargo build --release --locked)
flutter pub get
flutter build linux --release
bash packaging/test-make-deb.sh
bash packaging/make-deb.sh
```

If Flutter is installed outside `PATH`, use its full executable path. The
Linux package version is in `packaging/version`, independent of Android's
`pubspec.yaml` version; an explicit version can also be passed to the packager.
The package includes the engine and calculates dependencies from all bundled
native libraries. Build on the oldest distribution you intend to support;
the current artifact targets Ubuntu 24.04 amd64. Packaging does not install it.

See the [project README](../README.md) for build requirements, SSH setup, signing, secure storage and tests. Android workspace selection is per room; Linux project paths continue to configure the system CLIs.

Run `flutter analyze` and `flutter test` here. Native Android checks are in `integration_test/workspace_device_test.dart`; always build them with `ORG_GRADLE_PROJECT_orgbotQa=true` to use the separate QA package.

Room sessions belong to the app library, so navigation does not stop them. `RoomArchive` migrates older journals into the versioned native checkpoint; the Rust session persists complete engine and bot memory atomically. Android’s cached Flutter engine and `RoomService` retain active rooms while the activity is detached.

In Android room chat, **+ → Attach files** opens the native document picker: up to five files per message, 20 MiB each. Originals and attachment drafts stay in the room's private storage. Images are resized for vision models; text-only providers receive an explicit notice. PDF text extraction uses Android 15+; scanned pages and older Android versions use up to eight page images. DOCX, XLSX, PPTX, OpenDocument and UTF-8 files provide bounded text previews. Other formats keep their originals for inspection through the selected phone/SSH workspace. Layout, charts and unextracted pages may require the original; previews state their limits.

Attachment checks: `flutter test test/attachments_test.dart`, `cargo test --workspace` in `core`, and the isolated `integration_test/attachments_main.dart` phone target with the SSH fixture from `scripts/ssh-test-server.py`. The device check uses generated files and writes `files/workspaces/attachments-result.json` inside the QA package.

The shared Flutter interface uses bundled Inter (SIL OFL, in `assets/fonts/OFL.txt`), neutral light/dark surfaces, and native page/press/tab transitions that respect reduced-motion settings. Rooms and team searches retain their state across navigation; chat measurement remains outside visual animations. Run `flutter test test/polish_test.dart test/room_layout_test.dart test/chat_scroll_test.dart` for navigation, enlarged text and scroll anchoring checks.

Android rooms have no automatic wall-clock cutoff. Native turn/error limits still apply; exhausting a configured budget pauses unfinished work and records the reason. Linux CLI builds keep their own defaults. SSH probes idle connections before new operations, retries disconnected reads once against the saved host fingerprint, and keeps the transport available when a bounded command times out.
