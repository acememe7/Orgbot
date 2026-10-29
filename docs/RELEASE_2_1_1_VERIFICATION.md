# OrgBots 2.1.1 verification

Release date: 29 October 2026. Target: Ubuntu 24.04 amd64.

## What changed

- **Handoffs wait for a busy bot.** In the 2.1.0 live room, 13 of 15 handoff
  offers expired. A handoff's timeout now starts only once the bot whose move
  it is has begun a fresh turn since the handoff reached it. Until then it is
  held open while that bot is mid-turn or waiting for a free slot.
- **Accepted work is not cut off.** Accepting a handoff restarts its timeout,
  and the handoff stays open for the rest of the turn that took it on, so a bot
  doing the work asked for can still deliver it.
- **No offers that can never be accepted.** A delegation is withdrawn, with a
  reason, when its sender releases the task or loses its claim. Bots are told
  to keep holding a task until a delegation is accepted.
- **Trash helper tests** no longer depend on the owner's real trash settings or
  on thread timing.

Offline or faulted bots hold nothing open. A bot that has seen an offer, or
accepted one and ended its turn without finishing, cannot keep its sender
waiting for as long as it stays busy: the normal timeout runs from its next
turn.

## Release checks

- Full Rust workspace (`--locked`): 918 passed, zero failures, three ignored.
  Each new handoff test fails when its part of the fix is removed.
- Full Flutter tests: 363 passed, five skipped. Flutter analysis: no issues.
- Workspace sandbox helper: 66 passed. Trash helper: 30 passed.
- Linux acceptance: 17 passed, zero failed or skipped.
- Package build test, website checks (6 pages) and website consent test passed.
- Dependencies are unchanged from 2.1.0 (`Cargo.lock` is identical).
- The owner's real sign-in files were fingerprinted before and after every test
  run and were unchanged.

Final engine SHA-256:
`57a9b6200981a3f0c390599ae4747952095f0817fe8dc546d85b27761f258f25`.

| Package | SHA-256 |
| --- | --- |
| orgbots_2.1.1_amd64.deb | `214d39a324dad01acdc13624a73cf6bc77b2b043f6e94937e0649c95fcf05f6b` |
| orgbot_2.1.1_all.deb | `2bce8a9cd83516c8719f88b98c325b25d1df462a51cc815ffe3d8a49acf64d06` |

## Known limits

- A bot that accepts a handoff and ends its turn without finishing it is not
  reminded or woken for it in later turns. Its sender is released one timeout
  after that turn.
- Extending a handoff's deadline is not announced as a room event. The app does
  not show handoff deadlines, so nothing visible is stale.
- The limits recorded for [2.1.0](RELEASE_2_1_0_VERIFICATION.md) still apply,
  apart from the handoff expiry it lists.
