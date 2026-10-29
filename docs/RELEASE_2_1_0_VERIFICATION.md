# OrgBots 2.1.0 verification

Release date: 29 October 2026. Target: Ubuntu 24.04 amd64.

## What changed

- **License.** 2.1.0 and later are licensed under the PolyForm Noncommercial
  License 1.0.0, with the required notice in `LICENSE`. Releases 2.0.3 and
  earlier remain under Apache License 2.0. Crate metadata points at the license
  file instead of an SPDX identifier.
- **Bots working side by side.** Free slots are filled for the whole round, not
  only after the slowest turn. A bot goes again after a productive turn, at
  once when the owner asks it something, and otherwise after two minutes. Work
  reserved for a bot mid-turn is not handed to a second bot, so two inspectors
  are no longer woken for one review. The room is saved every two minutes while
  turns run.
- **Claims that hold.** A bot keeps its task while a long turn runs, while it
  waits on a teammate, and while it waits for a free slot. Unblocking renews
  the claim.
- **Messages and answers in real time.** Owner messages, decision answers and
  answers to a waiting bot land while turns run, without interrupting them.
  Owner input is journaled until the next save. A turn cut short no longer marks
  messages as read. Events go out once, desktop alerts go out as they happen,
  and a reconnecting app no longer sees answered questions again.
- **Pictures in the chat.** Images a Codex bot generates are posted from that
  bot as room attachments with a picture preview. Links to project files open in
  the Project tab, and linked project images show as thumbnails. Images whose
  declared size exceeds 40 megapixels are not decoded.
- **Sandbox.** Bots can use the machine's graphics card. Known secrets in the
  owner's home are hidden from bot runtimes: SSH and GPG keys, CLI sign-ins,
  cloud and registry credentials, browser profiles, shell histories and
  OrgBots' own data. Bots reach servers only through a bot-only SSH folder.
  `orgbot-trash move` works inside a bot's working copy; the original still
  needs owner approval before it moves to the owner's trash.
- **Providers.** Effort levels for Claude, Codex, Grok and Antigravity; no
  default turn time limit; refreshed sign-ins handed back to the owner only when
  valid and newer; prompts that begin with a dash no longer break the Claude and
  Grok CLIs; Codex room tool calls are named in the activity feed.

## Live proof

One real project room ran overnight on 28 to 29 October with 10 bots on 5
providers, on development builds and then on the 2.1.0 package. The per-bot
usage, delivered work, account dashboards and limits are in the
[live room record](benchmarks/2026-10-29-live-room-run.md). The final package
ran in that room from 03:07 to 04:11 before publication, and its engine is the
binary recorded below. In that hour eight bots took 34 turns, no claim was
released by mistake, one more task was delivered after review, and one turn
failed on a temporary Codex capacity error before the bot recovered.

## Release checks

- Full Rust workspace (`--locked`): 907 passed, zero failures, three ignored.
- Full Flutter tests: 363 passed, five skipped. Flutter analysis: no issues.
- Workspace sandbox helper: 65 tests passed, including real bubblewrap checks
  that bot runtimes cannot read the owner's keys, sign-ins or room tokens and
  can reach the graphics devices.
- Linux acceptance: 17 passed, zero failed or skipped, including private D-Bus.
- Package build test, installer test, website checks (6 pages), website consent
  test, website deploy unit tests, Pages index test and the APT builder test
  (signature, freshness, tamper, download and isolated install) passed.
- RustSec scan with cargo-audit 0.22.2: 233 dependencies, zero vulnerabilities,
  zero warnings.
- gitleaks 8.30.1 over the release source found three known false positives: a
  fake key in a test that checks project memory refuses secrets, a file digest
  in the 2.0.3 runtime report, and the public archive key fingerprint in the
  installer. Package text scanning covered 237,422 bytes with no findings.
- The owner's real sign-in files were fingerprinted before and after every test
  run and were unchanged.

Final engine SHA-256:
`5664d492c2b5d29c5ae9293486e0668a1cfe750c53e36ba9508b651f84224755`.

| Package | SHA-256 |
| --- | --- |
| orgbots_2.1.0_amd64.deb | `caaec9c402a3b431f3eedf020531322f6ed3cdca637cbafbf1fa3431bd611ee4` |
| orgbot_2.1.0_all.deb | `e30b7cbc415cde0ba897fcda9a64c74d35276212987303c28f3492b5830886a2` |

## Publication

The owner chose to verify this release personally instead of a bot review.
Source revision `8a4227dcee77565cd4f5ce40c2e20f0fca8cccdc` was pushed to
`main` and `master`. Signed archive revision
`347424fccca656f969b0c91940d81b55570ba1eb` keeps the 2.0.0 to 2.0.3 packages
byte-identical to the previous archive and adds 2.1.0. It was pushed and
verified before
[Pages run 40811091881](https://github.com/acememe7/Orgbot/actions/runs/40811091881)
was dispatched; its log confirms both exact revisions and good signatures.
Publication succeeded.

A fresh isolated APT refresh against the public HTTPS archive verified the
signature, selected 2.1.0, solved the upgrade from an installed 2.0.3 with no
removals, and downloaded both packages. Their hashes exactly match the table.

Website release `8a4227d` was activated only after public package
verification. All 18 public files match source byte for byte, including the
custom 404, and the active vhost matches source. Nginx validation and reload
passed with the four pre-existing shared-vhost warnings. HTTP redirect, HTTPS
routes, security headers and direct-origin denial passed. Live browser checks of
the home, results and install pages at 375 and 1440 pixels found no document
overflow, broken images or console errors. Both GitHub website and installer
checks passed. The previous `f1931cf` release and vhost are retained for
rollback.

A snapshot of the deployed site and vhost restored all 19 manifest entries into
a fresh isolated directory. The 281,025-byte archive has SHA-256
`1923ece57670765b4fe164dd38dcfb916f118b2ab3cf662e610571d4303cc2a8`. It is kept
on the owner's machine; the upload to the owner's private Drive folder is
pending.

The staged release diff passed secret scanning with no findings.

## Known limits

- A faulted bot is retried after one minute when the room is otherwise idle.
  While long turns run, the retry waits for another turn to finish; in the live
  room a Codex capacity error took eleven minutes to clear.
- Generated images are collected from Codex only. Grok's image output folder has
  not been verified, so Grok images are not collected yet.
- Hidden secrets are a list of known locations, not a full boundary: other
  files in the owner's home, such as a project's `.env`, stay readable to bots.
- The isolated upgrade harness used for 2.0.3 is not in the repository and was
  not repeated. The owner's machine upgraded from development builds to the
  2.1.0 package with the live room's data intact.
- Handoffs expire on a deadline the sending bot sets, even while the
  recipient is in a long turn on other work. In the final hour two of four
  handoffs expired before the recipient acted, and the senders re-offered or
  released the work instead.
- Room access to servers is not gated by the task board. In the final hour a bot
  with the bot-only SSH key installed a server release while other bots had put
  that release on hold; the owner chose to keep it live.
- Token counters do not establish invoice cost or quota use.
