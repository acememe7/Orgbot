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
`libgtk-3-dev`, `libsecret-1-dev`, `libwebkit2gtk-4.1-dev`, `libvte-2.91-dev`,
and `dpkg-dev`:

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
The `orgbots` package retains `/usr/lib/orgbot`, `/usr/bin/orgbot`, the
`orgbot.desktop` launcher, and existing per-user data while also providing an
`orgbots` command. Packaging also emits an `orgbot` transitional package that
depends on the renamed package, so apt upgrades existing installations. The
package includes the engine and calculates dependencies from all bundled
native libraries. Build on the oldest distribution you intend to support;
the current artifact targets Ubuntu 24.04 amd64. Packaging does not install it.

APT repository generation is described in [the distribution guide](../docs/apt-distribution.md).

See the [project README](../README.md) for the product overview and Linux build. Android setup, SSH workspaces, signing and device checks are below. Android workspace selection is per room; Linux project paths continue to configure the system CLIs.

Run `flutter analyze` and `flutter test` here. Native Android checks are in `integration_test/workspace_device_test.dart`; always build them with `ORG_GRADLE_PROJECT_orgbotQa=true` to use the separate QA package.

Room sessions belong to the app library, so navigation does not stop them. `RoomArchive` migrates older journals into the versioned native checkpoint; the Rust session persists complete engine and bot memory atomically. Android’s cached Flutter engine and `RoomService` retain active rooms while the activity is detached.

In Android room chat, **+ → Attach files** opens the native document picker: up to five files per message, 20 MiB each. Originals and attachment drafts stay in the room's private storage. Images are resized for vision models; text-only providers receive an explicit notice. PDF text extraction uses Android 15+; scanned pages and older Android versions use up to eight page images. DOCX, XLSX, PPTX, OpenDocument and UTF-8 files provide bounded text previews. Other formats keep their originals for inspection through the selected phone/SSH workspace. Layout, charts and unextracted pages may require the original; previews state their limits.

Attachment checks: `flutter test test/attachments_test.dart`, `cargo test --workspace` in `core`, and the isolated `integration_test/attachments_main.dart` phone target with the SSH fixture from `scripts/ssh-test-server.py`. The device check uses generated files and writes `files/workspaces/attachments-result.json` inside the QA package.

The shared Flutter interface uses bundled Inter (SIL OFL, in `assets/fonts/OFL.txt`), neutral light/dark surfaces, and native page/press/tab transitions that respect reduced-motion settings. Rooms and team searches retain their state across navigation; chat measurement remains outside visual animations. Run `flutter test test/polish_test.dart test/room_layout_test.dart test/chat_scroll_test.dart` for navigation, enlarged text and scroll anchoring checks.

Android rooms have no automatic wall-clock cutoff. Native turn/error limits still apply; exhausting a configured budget pauses unfinished work and records the reason. Linux CLI builds keep their own defaults. SSH probes idle connections before new operations, retries disconnected reads once against the saved host fingerprint, and keeps the transport available when a bounded command times out.

## Android: standalone models, selectable computer

Android loads its bundled Rust library; it neither starts nor depends on a Linux CLI. Sign in to supported providers on the phone (Codex/Grok), or configure a DeepSeek or Ollama Cloud API key. Claude CLI subscriptions remain a desktop capability. Bot keys, sign-ins and SSH credentials are stored in encrypted platform storage, with migration of old plaintext preferences after a successful secure write.

Grok subscriptions use the [Grok CLI catalog and chat proxy](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/README.md); API keys retain the public xAI API route. Access tokens renew before expiry, including while a native room is active. A rejected catalog request gets one renewal attempt; network and permission failures retain the saved sign-in. Mobile bots require a concrete model ID, with older empty selections repaired from the account's catalog.

Streaming model turns finish on the provider's completion event instead of waiting for the HTTP connection to close. Truncated streams cannot execute incomplete tool calls. Requests allow five minutes of network inactivity and thirty minutes overall; pausing the room cancels a request immediately.

Create or edit a room and tap **Working computer**:

- **This phone:** a separate persistent folder per room, with text-file list/read/write/directory tools. A custom absolute folder must already be accessible to the Android app. **Room options → Workspace files** lets you inspect/edit files and import/export UTF-8 documents with Android's document picker.
- **SSH / VPS:** enter host, port, username, existing absolute project directory and a password or OpenSSH/PEM private key (optional passphrase). For a key, use **Import key file** or **Paste key** to preserve its line breaks. The matching public key must be authorized for that server account. A VPS console password works for SSH only if the server permits SSH password login. Verify the server fingerprint independently, then use **Test connection and save**. Future connections require the saved key; a changed key fails closed. The remote machine needs SSH and SFTP, plus a POSIX shell if commands are enabled. It needs no OrgBot installation or model CLI.

Every bot in a room receives that computer's identity, root and available tools. File changes can be disabled. Shell commands require a separate switch and run with the SSH account's permissions; the working folder is **not a shell sandbox**. Use a restricted account when that is the intended access. File tools reject parent traversal and symlink escapes. Text files are limited to 1 MiB, listings to 500 entries, command output to 64 KiB, and commands to five minutes by default. Bots can set `timeout_seconds` from 1 to 1800 for longer builds or tests; a command timeout closes its channel while keeping SSH usable. Reads retry once after a connection loss; writes and commands are never automatically replayed. Existing files are replaced using a staged rename while retaining their basic permission bits; new files start private (0600). Servers without overwrite-rename support refuse replacement rather than truncate the original.

For an existing room, **Test connection and save** persists its computer settings immediately. A dropped SSH transport reconnects before the next tool call using the pinned server identity. The room shows connection status and a **Reconnect computer** action. No interrupted command or write is replayed automatically.

Set **Bots & rooms → Your profile** to choose the name and context sent to bots in new runs. Type **@** in the room composer to pick a bot from the list (arrow keys, then Enter or Tab), or type `@handle`. `@all` (also `@everyone`) asks every bot, and each one replies. A message with no tag goes to one bot, usually an available coordinator. Tags direct attention while the transcript remains visible to the whole room.

Pause interrupts the model turn and closes the workspace connection. Resume verifies/reconnects the selected computer. Failed writes and commands are never automatically retried: inspect their effects first. Active Android rooms use a foreground service while the app is in the background; force-stopping the app still stops execution. Saved rooms reopen paused with their history. The Work board shows blocking reasons and offers **Reply to unblock** for tasks waiting for your answer. Start failures remain visible across all room tabs.

Build an **arm64 development APK** and install without clearing app data:

```bash
# From repository root; Flutter, Rust, Android SDK and NDK must be installed.
rustup target add aarch64-linux-android
export PATH="$HOME/.cargo/bin:$PATH"
(cd app && flutter pub get && flutter build apk --debug --target-platform android-arm64)
adb install --user 0 --no-streaming -r app/build/app/outputs/flutter-apk/app-debug.apk
```

Gradle builds and copies the current Rust engine before packaging, and checks the required workspace ABI symbol. Set `ANDROID_NDK_HOME` if NDK discovery needs an override. Android requires API 24+. Release APK/AAB builds require your own `app/android/key.properties` (`storeFile`, `storePassword`, `keyAlias`, `keyPassword`); production builds cannot silently use the debug signing key. Keep that file and the keystore private.

Android shows an ongoing notification while rooms are active, with a **Pause all** action. A foreground service and wake locks keep the engine and SSH connections running when you leave the app or turn off the display. Pausing the last room releases those locks. Android force-stop, reboot or process termination still stops execution; saved rooms remain available for explicit resume. Linux continues to use the system engine and model CLIs.

In **Accounts**, save a DeepSeek or Ollama Cloud API key once; new bots reuse their provider's key from encrypted storage. Create an Ollama key at https://ollama.com/settings/keys, then choose **Ollama Cloud** when adding a bot. Models load from the live cloud catalog, and requests go directly to Ollama over HTTPS; no local Ollama installation is needed. Model availability and usage remain subject to your provider account. A single existing DeepSeek per-bot key migrates automatically. Distinct legacy keys are preserved. In room settings, **Working computer → Browse folders** loads the phone or SSH folder contents before you select a working path; **Files** opens that saved folder.

## Android device checks

`app/integration_test/workspace_device_test.dart` exercises the actual native FFI, phone file tools, file viewer and real SSH operations, including encrypted private-key authentication. Use the isolated fixture in a temporary Python environment with `asyncssh` and `bcrypt` installed. Start it in one terminal from the repository root:

```bash
python3 scripts/ssh-test-server.py
```

In a second terminal, also from the repository root, substitute your device ID
and the absolute `device-defines.json` path printed by the fixture:

```bash
adb reverse tcp:22222 tcp:22222
(cd app && ORG_GRADLE_PROJECT_orgbotQa=true flutter test integration_test/workspace_device_test.dart \
  -d 'DEVICE_ID' --no-uninstall \
  --dart-define-from-file='/absolute/path/to/device-defines.json')
```

Integration-test targets are refused unless `orgbotQa=true`, protecting the normal app package. The QA property uses `com.orgbot.app.qa`, keeping device tests separate from the normal package. Verify the reverse mapping remains active during the test (ADB reconnection clears it). Use `--no-uninstall`: Flutter otherwise removes its test package on completion. These deterministic tests do not validate a real paid model account or your own VPS configuration.

If Wi-Fi ADB repeatedly loses the test debugger, `integration_test/smoke_main.dart` runs phone-file, native-engine/tag, room navigation/restart and pinned-SSH-reconnect checks without a debugger. During its `background window` stage, background the QA app and turn off the display; `heartbeat.json` records progress and a native message plus an SSH reconnect run during that window. Reopen the QA app after 34 seconds to capture the restored chat and Memory screen. Build it with the same QA property and fixture defines using `flutter build apk --profile --target-platform android-arm64 -t integration_test/smoke_main.dart`, then install the QA APK, restore `adb reverse tcp:22222 tcp:22222`, and launch `com.orgbot.app.qa/com.orgbot.app.MainActivity`. Results are in the QA app’s `files/workspaces/smoke-result.json`. This smoke check supplements the widget/integration suite.
