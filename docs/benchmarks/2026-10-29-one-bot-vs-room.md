# One bot versus a reviewed room

Date: 29 October 2026. Data: [2026-10-29-one-bot-vs-room.json](2026-10-29-one-bot-vs-room.json).

## Question

Does a room with an independent reviewer ship fewer bugs than the same model
working alone? The reviewer did not write the code, so it does not trust it.

## Setup

The task was real work on the owner's private multiplayer battle royale game:
public party queueing for the battle royale mode, with its leave and cancel
rules and the web client's cancel. Both arms started from the same commit in
separate copies, received the same written task, and could not reach a server.

- **One bot:** Sol, Codex CLI `gpt-6.1-sol`, alone.
- **Room:** the same Sol implementing; Astra, Codex CLI `gpt-6-astra`, reviewing
  read-only; and a Claude Haiku 5.5 coordinator that submits only after approval.

Ten hidden checks were written before either run and never shown to the bots.
Eight test the new behaviour and two guard behaviour that must not change. They
were validated first: on the base commit the eight fail and the two pass; on an
earlier reviewed fix of the same feature from the owner's live room, nine pass
and one fails, confirming a bug that fix introduced.

## Result

| | One bot | Room |
| --- | ---: | ---: |
| Hidden checks passed | **3 of 10** (1 of 8 new behaviours) | **10 of 10** |
| Project tests | 822 passed, 0 failed | 846 passed, 0 failed |
| Typecheck | clean | clean |
| Delivered after independent approval | no | yes |
| Time | 31.5 min | 96.4 min active |
| Turns | 1 | 19 (Sol 7, Astra 4, Lead 8) |
| Output tokens | 50,527 | 138,800 |
| Uncached input and cache write | 186,043 | 616,921 |
| Cache read | 9,875,712 | 28,475,926 |

The one bot's work passed its own tests and the project's full suite, yet the
hidden checks found five defects:

- A member leaving a waiting public party left the rest of the party queued.
- Cancelling an offered party kept its seats reserved.
- A cancelled party could not queue again, and its old tickets stayed counted against the match.
- Public parties still required a private launch over HTTP, contrary to the task.
- The web client was not changed, so a follower's cancel never reached the server.

In the room, the reviewer found the same gaps before anything shipped, then
found new bugs in the fixes:

- First review, at 31 minutes: 5 blocking findings, including the 4-player refusal, the
  whole-party drop, requeueing after cancel, and a match still offered to new
  parties after it locked.
- Second review, at 62 minutes: 2 cancellation bugs the first fix introduced.
- Third review, at 81 minutes: 1 remaining bug, an expired member leaving without releasing
  teammates' seats.
- Fourth review, at 99 minutes: approved. The coordinator submitted.

## Cost

The room used 2.7 times the output tokens and about three times the time.
Codex carried almost all of it (134,183 of 138,800 output tokens); the Claude
coordinator used 4,617. This experiment used two providers; the overnight five-provider
room is a [separate record](2026-10-29-live-room-run.md).

## Limits

- One task and one run of each arm; a strong result for this task, not yet a general measurement.
- Both arms ran at the same time on the same Codex account while the owner's live room was also running.
- The room reached its 90-minute cap with its third revision awaiting review; it was resumed from its saved board without a time limit and finished 6.5 minutes later with no further code change. The code that passed all checks is the code it had at the cap.
- Token counters are as each CLI reported them; they are not invoices or quota readings.
- Bot sandboxes mount the computer read-only; no log or board shows either arm reading anything outside its own copy.
