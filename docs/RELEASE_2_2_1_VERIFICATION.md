# OrgBots 2.2.1 verification

Release date: 31 October 2026. Target: Ubuntu 24.04 amd64. Built from source
`0a89b21` on top of 2.2.0.

## What changed

- **No more crash at the room's save.** In the live room the room's engine
  panicked twice on 31 October, at 21:24:31 and 21:43:19, with "key must be a
  string" (both in the service journal). 2.2.0 kept waiting file removals in a
  map keyed by (bot, copy) pairs, which JSON cannot hold, and the save that
  tried to write it unwrapped the error. The first save after a bot left
  removals waiting took the room down. The service kept answering pings and
  serving the last state, so the app showed the room active while every owner
  message failed with "room control queue is full or closed", and each restart
  lost what came after the last good save. Waiting removals are now saved as
  `[bot, copy, count]` rows, and older boards still load.
- **A failed save no longer kills the room.** It fails on its own and pauses
  the room with a message. If the room's task dies anyway, its service exits
  with an error, so the supervisor restarts it from the last save and clients
  reconnect.
- **Handoffs survive a pause.** An assist offered just before the owner paused
  to install an update ran out five minutes into the pause. While the room is
  paused, each pending handoff keeps its whole timeout from the moment the room
  runs again.
- **You see who a message reaches before you send it.** The 20:58 update was
  meant for every bot but mentioned two bots in passing, so by the room's rules
  it reached only those two. Under the composer the same rule now says who the
  draft reaches: every bot, only the bots named, the bot replied to, or one
  coordinator.
- **Not sent, instead of Pending for ever.** A message the room does not take
  (its queue full or closed, or no confirmation in 30 seconds) shows "Not sent"
  with Send again and Discard.
- **Faster workspace commands.** Every command re-read and hashed a bot's whole
  5 GB copy, and the project, before and after it ran. Scans now remember each
  file's hash with its size, modification time, change time, inode and device,
  and read a file again only when one of those differs. A bot can set the
  modification time back but not the change time, so every edit is still seen.
- **Folders go with their files (rebuilt 1 November).** Approving the live
  room's cleanup left `.ng/` as 87 empty folders, with `.nongas/` and
  `.freeze-probe/` beside it, and two records disagreed about whether they were
  gone. Folders an approved removal leaves empty are now removed too, in the
  project and in the bot's copy.
- **Removal counts per copy (rebuilt 1 November).** The badge said 2,177 and the
  owner could not tell which bot owned them; 1,588 were in a copy that had none
  left. The review now names each copy with its count, opens on the copy with
  the most, and re-checks every other non-zero count.
- **Collapsible task cards.** Each card in the task list folds to its state, a
  one-line title and its owner, and opens with a tap; Collapse all and Expand
  all switch the whole list. A task waiting on the owner's reply, or parked,
  starts open.

## Release checks

- Full Rust workspace (`--locked`): 974 passed, zero failures, three ignored.
  New tests: a room with waiting removals saves and reopens, and an older board
  with an empty map still reads (fails on 2.2.0 with "key must be a string");
  a room with a sign-in waiting to be added saves; a handoff does not run out
  while the room is paused.
- Full Flutter tests: 397 passed, five skipped, at release; 398 at the
  1 November rebuild, adding per-copy counts and stale-count correction.
  Flutter analysis: no issues.
  New tests: who a message reaches (an update naming two bots reaches only
  them; `@all`, `@everyone` and `@everybody` reach every bot; no mention reaches
  one coordinator or the bot replied to; an unknown name reaches nobody), a
  message the room did not take shows as not sent and can be discarded, and
  task cards fold and open.
- Workspace sandbox helper: 86 passed at the 1 November rebuild (84 at
  release), adding folder cleanup after approved removals in the project and
  in a copy; including unchanged files not being read
  again while an edit with its modification time set back is still seen, and
  the hash cache kept private (mode 0600). Trash helper: 31 passed.
- Package build test passed. The 2.2.0 and older packages in the archive are
  byte-identical to those already published.
- The owner's real sign-in files were fingerprinted before and after every test
  run and were unchanged.
- The Linux acceptance suite was not rerun for this release.

Final engine SHA-256:
`74697632a2def43299860a1f0064dedbd4a22a179a3a916f12550dcc4b770698`.

| Package | SHA-256 |
| --- | --- |
| orgbots_2.2.1_amd64.deb | `e8500d128dfb7ecd0d4e6afcb2ea2f632632389ecbcf40b57426f643690da224` |
| orgbot_2.2.1_all.deb | `58d27d59d5cfd8c2456a7586964ef9a668a489b262e7979854b954d18f6ea4a9` |

## Publication

The owner asked for this release. Signed archive revision
`f993303ff3c7634bef017498bda312b67a17ac06` adds `orgbots_2.2.1_amd64.deb` and
`orgbot_2.2.1_all.deb` beside the earlier releases;
[Pages run 41242141639](https://github.com/acememe7/Orgbot/actions/runs/41242141639)
published it. The release commits passed secret scanning with no findings.

On 1 November the package was rebuilt under the same version with folder
cleanup after approved removals and per-copy removal counts (source `ed5b5fc`;
the engine is unchanged, the Flutter bundle and sandbox helper rebuilt). Signed
archive revision `2c7afe7a39dba8336aa8c4437bdeeafe98152641` replaces only
`orgbots_2.2.1_amd64.deb`;
[Pages run 41254171933](https://github.com/acememe7/Orgbot/actions/runs/41254171933)
published it, and the public archive serves the hashes in the table above.

## Known limits

- A room saves at the end of each batch of turns and on a timer during long
  ones; events after the last save are still lost if the engine stops
  unexpectedly. 2.2.1 removes the cause seen in the live room and restarts the
  service when it happens.
- The first scan of each copy after installing 2.2.1 still reads every file
  once, to build its hash cache.
- Verify copies share files; copies a bot can write do not, because ext4 has
  no copy-on-write.
- The website still names 2.2.0 as the current package.
