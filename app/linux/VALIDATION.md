# Linux CLI workspace validation — 2026-10-25

The shared Flutter app is the Linux build. New desktop bots use Codex, Claude
Code, or Grok CLI sessions; Android retains its OAuth account flow. Existing
saved bots are preserved. Desktop Files uses the same project folder as the
CLI agents. No real user library or CLI credentials were changed during QA.

## Completed

- [x] Desktop setup, provider picker, model lookup, and room configuration
  bypass phone OAuth. Regression tests cover saved phone tokens and separate
  desktop/phone catalogs.
- [x] Setup discovery matches the Rust engine's PATH and home-directory search.
- [x] Installed CLI help checked: Codex 0.164.0, Claude Code 2.1.289, Grok
  1.0.13. Updated Claude read-only invocations to `dontAsk` and passed each
  Grok MCP allowance with a separate `--allow` flag; argv regressions pass.
- [x] `flutter analyze --no-pub`: no issues. Full Flutter suite: 214 passed,
  3 skipped because the external SSH fixture was not enabled.
- [x] `cargo test --workspace --locked`: 692 passed, 1 ignored. Live API/CLI
  test flags were unset, so the passing count does not establish live access.
- [x] Fresh Rust release engine and Flutter Linux release built. The four
  Flutter engine/continuity checks passed again against the rebuilt engine.
- [x] Packaging fixture and shell syntax checks passed. Packaging requires an
  engine and scans every bundled native library for distribution dependencies.
- [x] Extracted package contains the exact rebuilt app and engine. All native
  libraries resolve; RUNPATHs are relative to `$ORIGIN`, with no build-machine
  paths. Android's optional JNI library is absent from Linux.
- [x] Native GUI checked under isolated Xvfb, D-Bus, XDG data, and keyring:
  Setup at 1280×720 and 800×600, keyboard provider selection, bot creation,
  shared-project room creation, and saved library restoration after restarting
  the extracted package. No model turn was started. Software rendering was used.

## Artifact

- `app/orgbot_1.5.1_amd64.deb` — 13,572,486 bytes; built on Ubuntu 24.04.
- SHA-256: `b149e377f0f7d6ec42b56a12267ea9056d0891721cdd28ea8f15b9d4841e9515`.
- Dependencies include `libc6 >= 2.39`, GTK 3, and `libsecret-1-0`. This artifact
  does not target older distributions. Linux packaging has its own version;
  Android remains at the existing app version.
- Session logs and rendered captures: `/tmp/orgbot-linux-g8qMwR/` (temporary).
  Key logs: `flutter-tests-final3.log`, `flutter-analyze-final3.log`,
  `flutter-final-engine.log`, `rust-tests-final.log`, `rust-build-final.log`,
  `flutter-build-final2.log`, `packaging-final.log`, and `package-runtime.log`.

## Remaining checks

- [ ] Real authenticated collaboration across all three CLIs, including MCP
  calls, session resume, and actual tool permissions. Local CLI configuration
  and each provider's account access still affect execution.
- [ ] Install/upgrade the system package. The existing installed 1.5.0 package
  was left intact; 1.5.1 was extracted and run without installing it.
- [ ] Native Wayland/GPU behavior, external SSH fixture, and Android device
  acceptance. Android was not rebuilt in this Linux pass.

Widget-only engine tests report unavailable `path_provider` while unpacking
skills; their engine/history assertions pass. Native startup did not report
that plugin error. Existing Rust test warnings and Xvfb desktop portal/DRI3
warnings remain. `dpkg-shlibdeps` also reports staging-layout/`$ORIGIN` and
unversioned-plugin warnings; its explicit bundled-library search succeeded,
and extracted-package `ldd` and startup checks passed.
