# OrgBots 2.2.2 verification

Release date: 10 November 2026. Target: Ubuntu 24.04 amd64. Built from source
`fd5ccf8` on top of 2.2.1.

2.2.2 comes from three test rooms: the 2 November benchmark against solo
Claude Code and Codex runs, and two speed tests on 3 and 7 November. Each room
was watched turn by turn, and its fixes went into the next build.

## What changed

### The room works in parallel

- **Workspace calls run side by side.** One bot's build no longer holds up
  every other bot's reads and writes; in the benchmark, reads had averaged
  35 s and writes 64 s. Each bot's own calls stay in order, and landing and
  pushing take turns.
- **Bots are not held back after a turn of edits.** An agent CLI turn spent
  editing files counts as work. In speed test 1 such turns counted as
  "nothing done" and cost 132 minutes of two-minute waits.
- **Work in review does not idle its author.** A bot whose held work waits on
  others (in review, blocked or approved) is offered new work meanwhile.
- **The room's clock runs every 15 seconds.** Lapsed waits, second opinions,
  long-hold notes and review rechecks happen on time instead of when some turn
  ends, and a stalled room wakes when someone new can act.
- **Up to 16 bots work at once,** and the setting says when a team is bigger.

### Reaching and steering bots

- Agent CLI bots hear messages for them, and turn-time reminders, on their
  room tools' results. A CLI turn the owner has waited a minute on is ended,
  and the next one resumes the same session with the message.
- The owner can pause, resume or remove one bot from the Team panel. The
  Team panel shows each bot's turn time, its longest turn, the turn under way,
  and its time in workspace calls.
- A turn that runs 30 minutes while its bot holds nothing is ended and the bot
  benched until resumed. The 30 minutes count from when the bot last held
  work, not from the start of its turn.
- Reasoning is cut short only while teammates wait on the bot: an API reply
  that only reasons for 5 minutes, or an agent CLI 10 minutes without a tool,
  is stopped and told to act. With nobody waiting, a coordinator can think a
  plan through for up to 30 minutes.
- A task held 45 minutes while other work waits on it is raised with its
  holder and the coordinators.
- A bot whose model service refuses every turn for 5 minutes hands its work
  back to the board, and the coordinators are told.
- A workspace command with no output and almost no processor use for 5
  minutes is stopped as stuck, with its output so far.

### Reviews

- A first send-back goes straight to its author. A second opinion is asked
  only for a rejection or a second send-back, from a free reviewer first, and
  is not waited for past ten minutes of a running room.
- A recheck goes to the reviewers who asked for the fixes for 5 minutes, then
  to any free reviewer. A bot that can review reviews waiting work before
  taking new work, and the owner is told when reviews pile up while bots that
  cannot review sit idle.
- Reviewers block only on the acceptance criteria, the owner's bar, a failing
  check or a defect a user would notice. A major says what the reviewer saw;
  a failure in another task's code becomes a fix task for that code.

### Coordination

- Coordinators move a task to another role or bot with `amend_task`
  (`required_role`, `hand_to`). Only the named bot may then claim it, so moving
  work needs no copy of the task, no retire card and no benched bot.
- Retiring a task can name its replacement (`replaced_by`). What waited on it,
  by dependency or by a block, moves to the replacement; without one, the
  coordinators and holders are told who is still waiting.
- Coordinators are told to open the first tasks within minutes, split work
  into 10 to 20 minute tasks, name an integration owner who builds the pieces
  as they land with a fast end-to-end check, run the full check once at the
  end, report "aiming for" targets rather than chase them, and near the end
  open new tasks only for Done when failures.
- Work with no role goes to a free builder first, a stand-in for a role waits
  while a bot holding the role is free, and the roster says what each bot runs
  on and which bots write code.
- A task that passed review can be landed by the bot that holds it.

### Safety of the owner's project

- A bot that holds no task, review, accepted handoff or recent request from a
  person cannot change the project. Its workspace writes are refused with the
  reason, and an agent CLI's turn-end merge stays in its own copy until it
  holds a task.
- Screenshots and scratch files go under `.cache/orgbots/` in a bot's copy and
  are never merged. New images, empty files or a `project/` folder written
  into the project root are named to their author.

### Agent CLIs

- Codex bots keep write access on resumed turns (Codex 0.166 ran every resumed
  turn read-only).
- Grok bots with write access may run Grok's own shell inside their sandboxed
  copy. Before, the shell needed an approval nobody could give and Grok ended
  the whole turn. A CLI that ends a turn on a refused tool now says so.
- A Claude bot's refreshed sign-in goes back to the owner's file within
  seconds, and only when it is good for longer than the owner's. A turn that
  ended signed out takes the owner's current sign-in and tries once more.
- The owner's own MCP servers are not given to the bots.

### Speed of each turn

- Workspace preparation reuses remembered file hashes, a merge with nothing to
  move skips its rescan, and a bot's private home is re-copied only when the
  owner's folders changed. A hung helper is stopped after 20 minutes.
- Saving the room holds the shared engine lock for less time, and the room's
  summary is rebuilt only when something changed.
- Claude API requests cache the conversation, not only the system prompt, and
  bots are told to make independent tool calls together.

## Release checks

- Full Rust workspace (`--locked`): 1,055 passed, zero failures, three ignored.
- Full Flutter tests: 417 passed, seven skipped.
- Workspace sandbox helper: 96 passed. Trash helper: 31 passed.
- Package build test passed; the packaged engine matches the release build.
- The owner's real sign-in files were fingerprinted before and after the test
  runs and were unchanged.
- The Linux acceptance suite was not rerun for this release.

Final engine SHA-256:
`9f4513228384341afe4940bb9b3a556f67315d45bb64cf6e790989d7f6d94bc9`.

| Package | SHA-256 |
| --- | --- |
| orgbots_2.2.2_amd64.deb | `f91cbc23d9f18a2880b4545109795c7f89d56bc80cc4fa3a72bdea9a022de0e9` |
| orgbot_2.2.2_all.deb | `8151fc6333c8a734bfe613dbdc18c74fed7e0bc2739efac9850655e52fad7098` |

## Publication

The owner asked for this release on 10 November 2026. Signed archive
revision `797dea9d21608c3b374419d7ef09776d8bac208e` adds `orgbots_2.2.2_amd64.deb`
and `orgbot_2.2.2_all.deb` beside the earlier releases, whose package files are
unchanged; [Pages run 42781097811](https://github.com/acememe7/Orgbot/actions/runs/42781097811)
published it. After publication the public `InRelease` signature verified with
the archive key, the public `Packages` index listed both 2.2.2 packages with the
hashes above, both packages downloaded with those hashes, and the 2.2.1 package
still downloaded unchanged. The release source was pushed as `3bd0a91`.

The website names 2.2.2 as the current package from release `3bd0a91` on the
same day (see [website hosting](website-hosting.md)).

## Known limits

- The speed test 2 fixes (re-routing, retiring with a replacement, the bench
  rule, the no-task write rule and Grok's shell) are covered by tests but have
  not yet run in a full room.
- In speed test 2 the room had not finished when it was paused at 4 hours 38
  minutes; solo Claude Code took 3 hours 12 minutes and solo Codex 1 hour 1
  minute. The causes found are fixed in this release; the room has not been
  timed again on it.
- Bots cannot record the screen with sound from their sandbox, so a recording
  with the browser's speech voice has to be made outside it.
- A bot holding no task can still change the project when a person asked it
  something in the last hour.
