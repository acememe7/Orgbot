# OrgBots recoverable trash

OrgBots provides the `orgbot-trash` command for intentional file removal. It
moves files and directories into `/mnt/hdd/OrgBots Trash`, grouped by project,
and records the time of the move in a private manifest. Entries can be
restored for seven full days. The separate daily cleanup timer may permanently
remove a complete entry once it is at least seven full days old, which means
the first scheduled run on day eight or later. Source-file timestamps do not
affect retention.

```sh
orgbot-trash move --project "Project name" -- /absolute/path/to/file /absolute/path/to/folder
orgbot-trash list
orgbot-trash restore ENTRY_ID
orgbot-trash cleanup --dry-run
```

The scheduled service runs `orgbot-trash cleanup` without `--dry-run`. Cleanup
removes only complete entries whose manifest and payload match, whose age is
at least seven days, and whose entry contains exactly the expected manifest
and payload. Incomplete, malformed, changed, or unexpected content is left in
place for recovery or inspection. Restore refuses to overwrite an existing
destination. A verified `pending_source_removal` entry can be restored too;
that entry remains in trash for inspection because the source-side move may
have stopped partway through. `list` includes its source recovery directory
when one remains.

The command stops if `/mnt/hdd` is not mounted as the expected ext4
volume or the trash destination is not writable. It does not fall back to the SSD mountpoint directory. It rejects
symlinks, special files, nested mount boundaries, hard-linked source files,
protected roots, and paths overlapping the trash directory. If a move is
interrupted after copying, the source-side quarantine or verified trash copy
is retained as a pending entry and is never automatically purged.
Move inactive files; stop applications that are still writing or holding files
open first. The command rechecks content after quarantine and preserves
mismatches, but it is not a live-filesystem snapshot service. Restore preserves
file bytes and permission modes, but not ownership, ACLs, extended attributes,
or original timestamps.

This command is a supported workflow, not an operating-system interception
layer. It cannot redirect arbitrary `rm`, shell, application, or agent file
deletions. The Linux agent instructions tell agents to use this command for
recoverable removals and to ask the owner if it is unavailable or the HDD is
not mounted. Permanent deletion remains a separate cleanup-script operation;
it is not an OrgBots agent task.

The standalone script and user timer were installed on this machine on
2026-10-25. The hardened cleanup service completed successfully with an empty
trash root; the daily timer is enabled and active. Its first scheduled run is
2026-10-26 at 00:01:41 WAT (the timer uses a small randomized delay). No user
files were moved or purged during installation. The earlier installed script
was preserved before applying the mount-namespace correction.

Install locations are `~/.local/bin/orgbot-trash`,
`~/.local/lib/orgbot/orgbot-trash.py`, and the service/timer under
`~/.config/systemd/user/`. Check scheduling with
`systemctl --user status orgbot-trash-cleanup.timer`.
The timer runs independently of OrgBots and makes no model calls. A user
systemd session must be available; persistent scheduling catches up after an
inactive session. The GUI package does not install or enable this timer.

For another machine, the installer is `app/packaging/install-orgbot-trash.sh`.
It refuses to replace unrelated files, verifies the mounted HDD, creates the
private trash root, installs under `~/.local`, and enables the timer. Its fixed
HDD location and ext4 requirement must match that machine before installation.
