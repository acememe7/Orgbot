# OrgBots 2.2.3 verification

Build date: 13 November 2026. Target: Ubuntu 24.04 amd64. Built on top of
2.2.2.

2.2.3 comes from a full audit of the code on 13 November 2026, which looked
for bugs, flawed logic, security problems, dead code and duplicated code. Every
finding was fixed, or left with a stated reason (see Known limits).

## What changed

### Safety

- **A bot's git calls run inside its sandbox.** `workspace_git` used to run
  git as the owner in the bot's copy, where a bot-written git config could run
  commands outside the sandbox.
- **Sign-ins are handed back only when they really are a refresh** of the
  owner's own account, read once without following links, and kept with five
  earlier copies. Each bot is seeded only with its own CLI's sign-in.
- **The no-task rule holds.** Only a question a person typed opens it, and a
  review queue no longer lets a reviewer write to the project.
- Room sockets live in a private folder with a random name; abstract sockets
  (such as X11) are blocked in the sandbox on kernels that support it.
- Sandboxes no longer see the trash, the helper's state folder or earlier
  sign-in backups. bubblewrap is run from the system folders only.
- `orgbot pair --out` creates its file private from the start; the pairing
  field in the app is hidden and cleared after a failed import.
- The project-memory filter catches more key shapes and no longer refuses
  ordinary prose. External MCP servers no longer receive `API_KEY`, `TOKEN`,
  `PASSWORD`, `SSH_AUTH_SOCK` or `DATABASE_URL`.

### Reliability

- **Merges are journalled and resumable.** An apply interrupted after it moved
  files to the trash finishes or rolls back on the next run; a first copy that
  fails part way never leaves a half copy behind.
- **One panic no longer stops the room**, and a crashing room backs off before
  restarting instead of resuming paid turns every few seconds.
- **Stopping or deleting a room while it starts stops its service.**
- Owner controls given during a run (park, resume, pause, remove a bot) are
  journalled, so a crash does not undo them. Input that cannot be replayed is
  kept and the owner is told.
- A refused API sign-in is read again from disk and retried once.
- Claude's server-side fallbacks no longer run a declined model's half-made
  tool call, and every attempt's tokens are counted.
- Landing works for files stored through a line-end conversion or LFS; the
  land and push cards show each commit's author.

### Speed

- A bot that ended a turn with nothing done waits two minutes before another,
  across runs, unless something new arrives for it.
- Approved work no longer wakes its author in a loop.
- Long tool output and long histories are cut before they fill the model's
  context; a context-too-long reply is retried once on a shorter history.
- Every API reply that only reasons is cut short while teammates wait (it was
  OpenAI-compatible models only in 2.2.2), and so is a stream that goes quiet.
- Reading a file in a protected room no longer rewrites the copy's manifest.

### The board

- Wait cycles through dependencies are refused, and health reports work that
  waits on a task that failed or was dropped.
- Tasks handed to one bot that cannot take them, review panels whose reviewer
  left, and work sent back to a paused bot all go back to the board.
- Cancelled and replaced tasks no longer count against "N of M delivered".
- Fable 5.2 has a price, so rooms using it have a known cost.

### The app

- The keyring is read again before a save if it was locked at start, so a save
  can no longer erase the other sign-ins.
- Multi-line arrays in `~/.codex/config.toml` import correctly.
- Unsaved edits in the project pane survive a session switch; a rename no
  longer restarts a running room, and other edits ask first.
- Sign out of everything, forgetting a key and removing a skill ask first.

## Release checks

- Full Rust workspace (`--locked`): 1,107 passed, zero failures, three
  ignored. `cargo clippy --workspace --all-targets` reports nothing.
- Full Flutter tests: 428 passed, seven skipped. `flutter analyze` reports
  nothing.
- Workspace sandbox helper: 108 passed. Trash helper: 32 passed.
- Website check: 11 pages passed.
- The owner's real sign-in files were fingerprinted before and after the
  build and were unchanged.
- The Linux acceptance suite was not rerun for this release.
- The first build attempt could not compile the Linux app: this machine no
  longer had the libsecret and VTE development files (the private copy earlier
  builds used was gone). A package made from the previous app build was set
  aside, not kept. After the owner installed `libsecret-1-dev` and
  `libvte-2.91-dev`, the build was run again from source `49d903d`; the app
  built, and the packaged app matches that build.
- Package build test passed; the packaged engine matches the release build.

Final engine SHA-256:
`cdfbede036703dc1fffe08968e67e08428e7a8f20ee08bfdff01b1f7e4c0e87d`.

| Package | SHA-256 |
| --- | --- |
| orgbots_2.2.3_amd64.deb | `7d6f800d923b078451098fe1c823b27c67f17bf66b73c102bc146b5103702aac` |
| orgbot_2.2.3_all.deb | `3c1051706d5e7af79bbcb0ea421e77e1f1800385d6580b0d37272dfe9a2a58f3` |

## Publication

Not published. This is a local build; the APT archive and the website still
name 2.2.2.

## Known limits

- None of the 2.2.3 fixes has run in a full room yet.
- Left as they are, with the reason:
  - Unreachable engine commands (cancel, fail, set authorization) are kept:
    saved rooms may hold them, and cancel and the revision budget are product
    decisions. The two unsafe dormant paths now require a yes.
  - The Dart engine port, the old `desktop/` shell and other unused code are
    not deleted; removing them changes no behaviour and is the owner's call.
  - The API and CLI tool paths, and the three API adapters, are not merged
    into one; their behaviour differences that were bugs are fixed.
  - Unknown keys in a room file are still ignored silently; refusing them
    could stop older room files from opening.
  - Older apps (wire version 1) still connect; the version check now says when
    an app and a room do not match instead of calling the token wrong.
  - The APT archive's signature is valid for 90 days from each publish; the
    next publish renews it.
