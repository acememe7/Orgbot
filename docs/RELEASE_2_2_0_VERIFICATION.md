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
- **Coordinators can amend a task.** In the live room a coordinator waived a
  requirement in chat, the task still listed it, and the reviewer sent the work
  back for it again; the fix went through four review rounds partly for that. A
  coordinator now changes a task's acceptance criteria or brief with
  `amend_task` and the reason: the change is recorded in the room log, survives
  a restore, is shown on the task card at once, and the task's holder is told.
  Reviewers judge against the task as it stands.
- **Coordinators are woken to deliver only what can go out.** In the live room
  an approved goal waited on one subtask still in progress, and the
  coordinators who deliver were woken every few minutes, five times in twelve
  minutes for one of them, only to say nothing needed doing. Approved work is
  now ready to deliver once nothing under it is unfinished and everything it
  depends on has landed; until then coordinators see what it waits on and are
  woken when it is ready.
- **Parked work stays parked.** In the live room the owner said to leave
  launch work parked; it stayed on the board as waiting work, so a builder
  claimed it and the coordinator asked the owner about it again fifteen minutes
  later. Tasks now have a parked state. You park a task from its card, or a
  coordinator does on your word with `park_task`; only you resume it. A parked
  task has no holder, is never offered, claimed or worked, and no bot may ask
  you about it. Questions waiting on it are withdrawn from Needs you, unless
  another live task waits on the same question.
- **Bots see what others added before planning more.** In the live room two
  coordinators woke to the same owner message and each split it into tasks;
  the one whose turn began first created five duplicates fifteen minutes later,
  two bots took the same job, and the owner was asked four times to retire the
  copies. A bot creating a task is now turned back once with the open tasks
  others added since its turn began, and creates its task again only if it is
  still needed.
- **Bots can land their commits after one approval.** A bot's copy keeps its
  own git history, so in the live room a bot asked to commit could only hand
  the owner a bundle and a script. `workspace_land_commits` checks a bundle the
  bot wrote in the project, shows the owner the commits on one card (a second
  tap confirms; the access mode applies), and moves the branch forward only:
  no push, no merge, no history rewrite, no project file written, and only
  when the project already holds every changed file as the commits leave it.
  Asking the owner for access or a landing starts a task the bot had only
  claimed, so it waits on the answer instead of being woken while the owner
  has the card (in the live room a bot was woken eleven times that way).
- **Codex bots can start local servers.** Codex's own sandbox, inside
  OrgBots' one, refused every socket, so a Codex bot's browser captures failed
  with EPERM. A Codex bot allowed to write now has the network inside its CLI,
  like bots on other CLIs; writes outside the project are still refused.
- **Long reviews stay responsive.** A cleanup left 1,570 removals to review
  in the live room, and the app froze on apply: every path was built at once.
  The review and the delete confirmation now build only the rows on screen and
  open with how many files sit in each top-level folder.
- **No more project copy at every room start.** To inherit the computer's MCP
  servers, the room ran a settings lookup through the workspace under a fresh
  bot id at each start; in a protected local room that copied the whole
  project (4.6 GB in the live room) to read a file the sandbox hides, and the
  startup cleanup deleted it. The lookup now runs only on remote computers.
- **A lent GitHub CLI signs in from the keyring.** The GitHub CLI keeps its
  token in the desktop keyring, which a sandbox cannot reach, so in the live
  room a bot lent it got a 401 on push. The launcher now asks your `gh` for the
  token outside the sandbox and gives only that turn `GH_TOKEN`, with git set to
  use it. The token never appears on a command line or in a file.
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
  button that shows how many file removals wait for review, and each bot's two
  project copies are labelled "its own CLI" and "room tools".
- **Easier long runs (rebuilt 31 October).** Approved removals move to the
  trash in batches with a running count (1,549 had taken 13 minutes behind a
  bare progress bar). Bots no longer repeat waiting removals in every message,
  and a turn that changed nothing posts its status once. A coordinator can make
  a task wait for others, and waiting work is left alone. A bot asks the owner
  one question per task at a time, and asks for a step's access at once. A
  landing card names the checks recorded for its commits. A bot can push an
  existing branch of the owner's project after a card the owner always sees,
  whatever the access mode. The app says what happened while the owner was
  away, lists leftover copies with their size and discards one after saving
  what may be unique in it. Each copy's git history keeps up with the
  project, and verify copies share their large files.
- **Sign-ins bots can ask for (rebuilt 31 October, later).** Requests say why
  the work needs it, what the bot will do, the commands planned and what
  happens if denied, each on its own line of the card. A bot can ask for a
  sign-in that is not listed; that always asks the owner and, if allowed,
  joins their list. Settings knows 33 tools and shows which are signed in on
  this computer but not turned on. Switched-off sign-ins, the owner's own
  included, are hidden from every bot.
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

- Full Rust workspace (`--locked`): 973 passed, zero failures, three ignored,
  at the last 31 October rebuild.
  New tests cover landing a real bundle in a real repository (refused before the project holds the files, refused off the approval, for diverged commits, links and path-like branches; staged work kept), the approval covering exactly those commits, landing under each access mode, showing a bot the tasks others added during its turn, parking and resuming a task, withdrawing the questions waiting on it, when approved work is ready to deliver, amending a task, a bot resting after turns of talk, a lapsed claim waking a resting room, CLI tool-use counting, a command in a conflicting copy, the four access modes, a lent sign-in reaching a real CLI turn
  through the sandbox launcher, the lend list's limits,
  outside tool calls by mode, the server-key grant and its sandbox mount, per-turn access,
  accepted handoffs, retry timing, CLI error classes, owner paging, the graceful
  pause and held rooms, retiring tasks and leftover-copy cleanup; each was checked to fail without
  its fix.
- Full Flutter tests: 391 passed, five skipped. Flutter analysis: no issues.
- Workspace sandbox helper: 83 passed (trash helper 31) at the 31 October
  rebuild, adding batched removals, discarding leftover copies, history
  catch-up (including a symlink planted in a copy) and shared verify copies.
  Earlier: 73 passed, including a GitHub CLI token from the keyring reaching gh and git in the sandbox and never a command line, and setting a bot's conflicting files aside without a link redirecting them, including real bubblewrap checks that a
  turn the owner did not allow, or a read-only turn, has no server key anywhere,
  that lent sign-ins are hidden unless lent and then appear only in the bot's
  home, and that a once read-only workspace never gets the key. Trash helper: 30
  passed.
- Linux acceptance: 17 passed, zero failed or skipped.
- Package build test, website checks (8 pages) and website consent test passed.
- Dependencies are unchanged from 2.1.0 (`Cargo.lock` is identical).
- The owner's real sign-in files were fingerprinted before and after every test
  run and were unchanged.

Final engine SHA-256:
`77d02f119be0650a5fd95653220b9703700638e7b4df4b3a5ff45c6305aee159`.

| Package | SHA-256 |
| --- | --- |
| orgbots_2.2.0_amd64.deb | `11add74c2c4fd5257d785f92bd7e046f9dde47fd880afcbf1a5f6223e3c2e073` |
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
published it.

The next morning they were rebuilt again under the same version so a bot that
only talks is rested and a tick that frees work wakes the room (source
`0b128f3`; the Flutter bundle is byte-identical to the previous package).
Signed archive revision `9b09534a5b15a8bfe11b149e06a3763b303de30b` replaces
only `orgbots_2.2.0_amd64.deb`;
[Pages run 41030440577](https://github.com/acememe7/Orgbot/actions/runs/41030440577)
published it.

That evening they were rebuilt again under the same version so coordinators can
amend a task (source `6c69cdf`; the Flutter bundle was rebuilt to show amended
tasks). Signed archive revision `fc5a74366ed4c3a7bae00e0646b41f295a74c1b0`
replaces only `orgbots_2.2.0_amd64.deb`;
[Pages run 41102385617](https://github.com/acememe7/Orgbot/actions/runs/41102385617)
published it.

Later that night they were rebuilt again so coordinators are woken to deliver
only work that can go out (source `d1dacc8`; the Flutter bundle is
byte-identical to the previous package). Signed archive revision
`c7851effefd747695a03fa0a937b2ce749368857` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 41131413062](https://github.com/acememe7/Orgbot/actions/runs/41131413062)
published it.

Late on 30 October they were rebuilt again so parked work stays parked and a
lent GitHub CLI signs in from the keyring (source `1e3c268`; the Flutter bundle
was rebuilt to show, park and resume tasks). Signed archive revision
`686065df77400532e8bfdcf57328fd909db4f0db` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 41141310237](https://github.com/acememe7/Orgbot/actions/runs/41141310237)
published it. A real sandboxed turn lent the GitHub CLI was signed in to the
owner's account from the keyring; one not lent was signed out. In the live room
that night the owner parked launch work and the room spent no turns on it, and
a bot pushed the released game to GitHub with the lent sign-in.

On 31 October they were rebuilt again so a bot is shown the tasks others added
during its turn before it creates more (source `a800543`; the Flutter bundle is
byte-identical to the previous package). Signed archive revision
`dfa790179009ef8eb6d0ee62a7b1243868e94de3` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 41208959113](https://github.com/acememe7/Orgbot/actions/runs/41208959113)
published it.

Later that afternoon they were rebuilt again so bots can land their commits
after one approval (source `ac26558`; the Flutter bundle is byte-identical to
the previous package). Signed archive revision
`edf9a5a7c6194c21265437b2f1ebc0b1a20523b8` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 41211548830](https://github.com/acememe7/Orgbot/actions/runs/41211548830)
published it. Its first live use landed nothing at first: the bot asked for a
task it had only claimed, so the question reached the owner while the bot was
told it failed.

That evening they were rebuilt again so asking the owner starts a claimed task
and waits on the answer (source `60446e4`; the Flutter bundle is byte-identical).
Signed archive revision `4c3febc0fa03002e1bdab6513302572d9540a17a` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 41219081571](https://github.com/acememe7/Orgbot/actions/runs/41219081571)
published it. The first approved landing on it then reported success while the
owner's branch did not move: in a protected room it ran in the bot's own copy.

Within the hour they were rebuilt again so landing always acts on the owner's
project (source `349bc86`; the Flutter bundle is byte-identical). Signed archive
revision `6e4bcf6c0d356b1faf4e3a63ea98bab3ab0303a1` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 41220708972](https://github.com/acememe7/Orgbot/actions/runs/41220708972)
published it.

Minutes later they were rebuilt again so Codex bots that may write have the
network inside their CLI (source `e1aa139`; the Flutter bundle is
byte-identical). Signed archive revision
`0c1768eb0efcb855928606079af4f95b71c0d18a` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 41221352175](https://github.com/acememe7/Orgbot/actions/runs/41221352175)
published it. In the live room a Codex bot's capture run then reached the map
with no EPERM.

That evening the app was rebuilt so a long review of workspace changes stays
responsive (source `0903ec5`; the engine is unchanged). Signed archive revision
`bbf1f1bd86db798bae2de8ca6bfa8da74bce37c0` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 41223113034](https://github.com/acememe7/Orgbot/actions/runs/41223113034)
published it.

Then the engine was rebuilt so a room start no longer copies the project to
look for settings (source `ae36218`; the Flutter bundle is unchanged from the
previous package). Signed archive revision
`878a7adec112248aaeedec15fbba58102e31d998` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 41224108972](https://github.com/acememe7/Orgbot/actions/runs/41224108972)
published it.

On 31 October the package was rebuilt with the update for easier long runs
described above (source `832f6a3`; engine and Flutter bundle both rebuilt).
Signed archive revision `2aad39ceedd1eb276644beb1ccd62130b0daf4fd` replaces
only `orgbots_2.2.0_amd64.deb`;
[Pages run 41232249026](https://github.com/acememe7/Orgbot/actions/runs/41232249026)
published it.

Before the owner restarted, the sandbox helper was changed so copies made
before the update catch up too: in the live room all three of one bot's copies
sat on an old commit the project had long moved past (source `9dcc0f7`; engine
and Flutter bundle unchanged). Signed archive revision
`b11eedbf75ab89012ddf4384a05f1b809e5a3925` replaces only
`orgbots_2.2.0_amd64.deb`;
[Pages run 41233166006](https://github.com/acememe7/Orgbot/actions/runs/41233166006)
published it.

Still before the restart, the package was rebuilt so bots can ask for sign-ins
that are not listed and every request card says why, what, the commands and
what happens if denied (source `9c991cf`; engine and Flutter bundle rebuilt).
Signed archive revision `eeb7ec7ce4f164811fc6464d9166a2f073eb0ab0` replaces
only `orgbots_2.2.0_amd64.deb`;
[Pages run 41235683415](https://github.com/acememe7/Orgbot/actions/runs/41235683415)
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
