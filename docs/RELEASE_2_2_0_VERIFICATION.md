# OrgBots 2.2.0 verification

Release date: 29 October 2026. Target: Ubuntu 24.04 amd64. 2.2.0 includes the
handoff fix prepared as 2.1.1, which was never published.

## What changed

- **Servers only with your permission.** Bot sandboxes no longer carry the bot
  server key. A bot holding a task that needs a server calls
  `request_server_access` with what it will do; you see a decision and confirm
  "Allow server access for this task". Only that bot gets the key, only for
  that task, and only while the task is claimed or in progress, clear of holds
  and unfinished dependencies. Review-only and read-only bots never get it. The
  room decides afresh for every turn and every command, and the grant is the
  answered decision itself, so a restored room grants exactly what you allowed.
  The key folder is also hidden at its own path. In the 2.1.0 live room a bot
  had deployed a server release while the rest of the room held it.
  The 2.2.0 package was rebuilt the same day under the same version: in the
  live room the owner allowed Sol the key, but workspace commands a CLI bot
  sends through the room bridge still ran without it. The republished package
  passes the grant on that path too, with a test that fails without the fix;
  installations of the first 2.2.0 package need `apt install --reinstall`.
- **Access modes and lent sign-ins.** Each room has an access mode: Approval
  (the default) asks you for everything, Half approval approves regular
  requests, Auto approves everything but critical requests, and Full write
  approves everything while bots still write only in their own copy.
  Questions about the work always reach you. Bots ask with `request_access`,
  saying what they will use and do; a request is rated the higher of the action
  (release is risky, deleting or spending critical) and the risk you gave the
  tool. Approvals by mode are recorded as answered decisions. Outside tool calls
  count as risky. **Connections & tools → Sign-ins bots may use** lists tools
  such as Wrangler, the GitHub CLI and Vercel, all off until you turn them on;
  a lent sign-in is mounted into the bot's own home for that task only. In the
  live room every bot could read the owner's Cloudflare sign-in while the bot
  that needed it for a release had none. Deploy tools' sign-ins and the app's
  own support folder are now hidden from bots, and so is OrgBots' data where
  `XDG_DATA_HOME` moves it. A workspace that was ever read-only never gets the
  server key.
- **Pausing loses no work.** Pause now stops new turns and lets the bots already
  in a turn finish it, then pauses; the room shows "Pausing after current turns"
  until it is safe to close or update. "Stop turns now" in the room menu cuts
  turns short for emergencies. Before, a pause cut every running turn short, so
  updating a busy room meant waiting for a moment when no bot was mid-turn.
- **Stale tasks can be retired.** A coordinator proposes retiring an obsolete
  task with `propose_retire_task` and the reason; you answer "Retire this task"
  and it is retired, so it stops waking bots. In the live room an obsolete task
  woke idle bots every turn while the bots asked you to archive it in an app
  that had no way to.
- **Removing files from a working copy.** Bots are told to pass paths inside
  their working copy to `orgbot-trash`, and the helper says so when given the
  original project's path instead of sending the bot to configure your trash.
- **A conflicting copy no longer stops a bot.** In the live room a bot whose
  copy conflicted with files others had changed was told every command had
  failed, with its output thrown away, though each one had run. Its version
  of each conflicting file is now kept in its private home under
  `~/orgbot-conflicts/`, its copy takes the project's newer version, the rest
  merges, and the command's output always reaches it. A CLI turn reports a
  merge problem instead of failing. A lent sign-in that is not on this computer
  is skipped rather than stopping the turn.
- **A bot going round in circles is rested, and freed work is picked up.** In
  the live room a bot holding an urgent fix took 26 turns in an hour, each a
  single reply resuming a 140,000-token session, saying it was about to change
  the code and changing nothing. Its repeats put the room to sleep, and when
  the claim lapsed nobody was woken to take the fix until a person spoke. A bot
  that ends 3 turns in a row on its work without using a tool or changing a
  file (counting tools used inside an agent CLI) is no longer woken for that
  work until someone talks to it or the work changes; its next turn starts a
  fresh session, and the coordinators are told. A housekeeping tick that frees
  work (a lapsed claim, an expired handoff, a bot back from a fault) wakes a
  resting room.
- **Leftover copies are cleaned up.** When a room starts, working copies of
  bots no longer in it are removed if they hold no unmerged work; copies that
  might hold unsaved work are kept and named in the room. In the live room such
  copies had grown to 38 GB.
- **Answers that take effect need a second tap.** Server access, outside tool
  calls and retiring a task ask you to confirm; every other answer stays one
  tap.
- **Handoffs wait for a busy bot.** In the 2.1.0 live room, 13 of 15 handoff
  offers expired. A handoff's timeout now starts only once the receiving bot
  begins a fresh turn, accepting restarts it, and the accepting turn is held
  open. A delegation is withdrawn with a reason when its sender lets the task
  go. A bot is woken for, and shown, handoffs it accepted and has not delivered,
  and can give one up with `fail_handoff`. While that bot waits on your answer
  to its own question, it is not woken for them and they do not expire; in the
  live room such a bot was rerun nine times in 23 minutes while you had the
  question, and the assist it held ran out and was offered again.
- **Faster, clearer recovery.** A faulted bot is retried when its retry is due,
  not when another turn happens to finish (a Codex capacity error took eleven
  minutes to clear in the live room). A CLI "at capacity" or "overloaded" error
  is waited out instead of faulting the bot. A lost sign-in names the CLI to
  sign in to again.
- **Less noise.** A question from one bot to another no longer pages you, and
  bots post at most one short room message per turn. Bots may not put a status
  or a hold in your queue as if it were a question. A bot returning after a
  long absence is shown the newest 32 KB of what it missed, messages to it and
  from you first, with a count of what was left out. In the live room a bot
  away for eight hours was due 133 KB of catch-up, more than Linux allows in
  one command-line argument, so its CLI could not start.
- **Easier to find.** The workspace review is a labelled "Workspace changes"
  button, a bot asking for file removals says where it is, and each bot's two
  project copies are labelled "its own CLI" and "room tools".
- **Provider terms.** The Antigravity sign-in tile warns that Google's terms do
  not allow a personal Google account in third-party apps. The README
  summarises each provider's stated position.
- **License.** 2.2.0 and later are licensed under the PolyForm Shield License
  1.0.0: anyone may use it, including at work; nobody may offer a product that
  competes with OrgBots. 2.1.0 was PolyForm Noncommercial 1.0.0; 2.0.3 and
  earlier were Apache 2.0.

## One bot versus a reviewed room

The 2.2.0 development build ran the same real task, public party queueing in
the owner's private battle royale game, twice from the same commit. The same
Codex model worked alone in one copy and as the implementer in a room with an
independent read-only reviewer and a coordinator in the other. Ten hidden
checks, written beforehand and validated on the base commit, judged both.

- One bot: 3 of 10 hidden checks (1 of 8 new behaviours), although its own
  tests and the project's 822 passed. 31.5 minutes, 50,527 output tokens.
- Room: 10 of 10, 846 project tests passing, delivered after the reviewer's
  fourth review (5, 2 and 1 blocking findings, then approval). 96.4 active
  minutes, 138,800 output tokens.

One task and one run of each; see the
[full record](benchmarks/2026-10-29-one-bot-vs-room.md) and its limits.

## Release checks

- Full Rust workspace (`--locked`): 946 passed, zero failures, three ignored.
  New tests cover a bot resting after turns of talk, a lapsed claim waking a resting room, CLI tool-use counting, a command in a conflicting copy, the four access modes, a lent sign-in reaching a real CLI turn
  through the sandbox launcher, the lend list's limits,
  outside tool calls by mode, the server-key grant and its sandbox mount, per-turn access,
  accepted handoffs, retry timing, CLI error classes, owner paging, the graceful
  pause and held rooms, retiring tasks and leftover-copy cleanup; each was checked to fail without
  its fix.
- Full Flutter tests: 371 passed, five skipped. Flutter analysis: no issues.
- Workspace sandbox helper: 72 passed, including setting a bot's conflicting files aside without a link redirecting them, including real bubblewrap checks that a
  turn the owner did not allow, or a read-only turn, has no server key anywhere,
  that lent sign-ins are hidden unless lent and then appear only in the bot's
  home, and that a once read-only workspace never gets the key. Trash helper: 30
  passed.
- Linux acceptance: 17 passed, zero failed or skipped.
- Package build test, website checks (6 pages) and website consent test passed.
- Dependencies are unchanged from 2.1.0 (`Cargo.lock` is identical).
- The owner's real sign-in files were fingerprinted before and after every test
  run and were unchanged.

Final engine SHA-256:
`e8bf52a2304abc4ffba2e86b24c55a7142ab3aaf757f9c07a1c8007ab54b1f8f`.

| Package | SHA-256 |
| --- | --- |
| orgbots_2.2.0_amd64.deb | `2310be50c6ceb5a0f43c100a6abf9f264d2430a87c53c0e8af7078cff420f2c1` |
| orgbot_2.2.0_all.deb | `9e7bbfc696cb887195027910c830ee03c1e4a1714184125ead1a3a5f801fafef` |

## Publication

The owner approved publication. Source revision
`6b6ee9d39190fc178258d028a7df5ff91e913b7f` was pushed to `main` and `master`;
it includes the unpublished 2.1.1 handoff commits. Signed archive revision
`b15a677544570df818fafbec2a0bf4344b6313e7` keeps the 2.0.0 to 2.1.0 packages
byte-identical to the previous archive and adds 2.2.0 (2.1.1 was never
published). It was pushed and verified before
[Pages run 40912017444](https://github.com/acememe7/Orgbot/actions/runs/40912017444)
was dispatched; its log confirms both exact revisions and good signatures.

A fresh isolated APT refresh against the public HTTPS archive verified the
signature, selected 2.2.0, solved the upgrade from an installed 2.1.0 with no
removals, and downloaded both packages. Their hashes match the table.

Website release `6b6ee9d` was activated after public package verification. It
leads with the one-bot versus reviewed-room result and the 2.2.0 live-room
observations. All 19 public files match source byte for byte, including the
custom 404, and the active vhost carries the new structured-data hashes. Nginx
validation and reload passed with the four pre-existing shared-vhost warnings.
HTTP redirect, security headers and direct-origin denial passed. Live browser
checks of the home, results and install pages at 375 and 1440 pixels found no
overflow, broken images or console errors. GitHub website checks
`40911820438` and `40911821485` passed. The previous `8a4227d` release and
vhost are retained for rollback.

A snapshot of the deployed site and vhost restored all 20 manifest entries into
a fresh isolated directory. The 290,396-byte archive has SHA-256
`7dc05530b6f90ad1415f5124d1df1d6e067899b22957efc74bcaea49ff82edf4`. It is kept
on the owner's machine and in the owner-only `orgbots_backups` Drive folder,
where its size matches and only the owner has access. It was not downloaded
back for a hash comparison.

Later the same day the 2.2.0 packages were rebuilt under the same version
with the room-bridge access fix (source `e803d87`). Signed archive revision
`9612c8ccfd18c2642c9a62e8b85ca22a320406d8` replaces only the 2.2.0 packages;
[Pages run 40918122421](https://github.com/acememe7/Orgbot/actions/runs/40918122421)
published it.

That evening they were rebuilt again under the same version so a bot waiting
on the owner is not rerun for an assist it accepted (source `9e734a3`; the
Flutter bundle is byte-identical to the earlier package, only the engine
changed). Signed archive revision `0b1e04d0a9413b678fd24d5cdae56c8f33fdb696`
replaces only `orgbots_2.2.0_amd64.deb`; the transitional package and every
older package are byte-identical.
[Pages run 40955267257](https://github.com/acememe7/Orgbot/actions/runs/40955267257)
published it.

Later that night they were rebuilt once more under the same version with the
bounded catch-up, after the Grok bot faulted on "Argument list too long"
(source `f225c61`). Signed archive revision
`d0666190ec45466f27f0bdade3ccf9711770ce98` again replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 40966749910](https://github.com/acememe7/Orgbot/actions/runs/40966749910)
published it.

The next morning they were rebuilt under the same version with room access
modes and lent sign-ins (source `d25e9f3`; the Flutter bundle was rebuilt for
the new settings). Signed archive revision
`86893ae21a4fadafbcfbe9d7cc69437c4233ac05` replaces only
`orgbots_2.2.0_amd64.deb`; the transitional package and every older package are
byte-identical.
[Pages run 40984234732](https://github.com/acememe7/Orgbot/actions/runs/40984234732)
published it.

An hour later they were rebuilt once more under the same version so a bot
keeps working when its copy conflicts with the project, and so a lent sign-in
that is not on this computer is skipped (source `95d458a`; the Flutter bundle is
byte-identical to the previous package). Signed archive revision
`fe2788231a8d3c06acbd3db72f7fe9e50ae2b21e` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 40991917865](https://github.com/acememe7/Orgbot/actions/runs/40991917865)
published it, and the public archive serves the hashes in the table above.

The release commits passed secret scanning with no findings.

## Known limits

- A sandbox started with the server key keeps it until that turn ends, so a
  hold placed mid-turn applies from the next turn. Restrict the bot key in the
  server's `authorized_keys` for a server-side limit as well.
- The server-key gate controls the key OrgBots mounts. Network access itself is
  not confined, and other credentials in a project (such as a `.env`) stay
  readable to bots.
- A bot that accepts a handoff and ends its turn without finishing it is woken
  again; if it still does not finish, the handoff expires one timeout later.
- Extending a handoff's deadline is not announced as a room event; the app does
  not show handoff deadlines.
- Whether a provider permits its subscription to be used through OrgBots is set
  by that provider's terms, which change.
- In the live room on 2.1.0, two owner answers sent from the app did not reach
  the room for some minutes; the room itself answered at once and recorded
  them once they arrived. The cause was not found in this release.
