# OrgBots recoverable trash

OrgBots provides the `orgbot-trash` command for intentional file removal. It
moves files and directories into the selected recovery folder, grouped by project,
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
orgbot-trash config
orgbot-trash configure --root /absolute/path/to/private-recovery-folder
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

From 2.0.1, new installations default to `~/.local/share/orgbots/trash`, or
`$XDG_DATA_HOME/orgbots/trash` when an absolute XDG data path is configured.
Choose another location under **Connections & tools → File recovery** or with
the `configure` command. The parent of a custom folder must already exist;
the helper creates its private leaf. Existing folders must be user-owned with
0700 permissions. Configuration is stored in
`$XDG_CONFIG_HOME/orgbots/trash.json` (normally `~/.config/orgbots/trash.json`),
with 0600 permissions inside a private directory. Manual operations, workspace
integration and scheduled cleanup use that same selection.

Legacy installations with a standalone HDD helper or timer retain
`/mnt/hdd/OrgBots Trash` and its writable ext4 mount requirement. An upgrade
from an older Debian package without a recorded per-user selection asks the
owner to choose storage explicitly. No existing recovery files are relocated.
Switching away from a folder containing entries is refused; restore or resolve
those entries first. An unavailable current selection must be made available
before changing it.

The selected filesystem is recorded and checked on use. Missing, read-only,
replaced or unverifiable storage stops the operation without falling back to
another disk or writing into an unmounted mountpoint. The helper rejects
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
recoverable removals and to ask the owner if it or the selected storage is
unavailable. Permanent deletion remains a separate cleanup-script operation;
it is not an OrgBots agent task.

As historical installation evidence, the standalone script and user timer were installed on the owner's machine on
2026-10-25. The hardened cleanup service completed successfully with an empty
trash root; the daily timer was enabled and active. Its first scheduled run was
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

For another machine, explicitly opt into automatic cleanup by running
`bash app/packaging/install-orgbot-trash.sh` from the source checkout. It
preserves recognized older managed files before upgrading them and refuses to
replace unrelated files. It validates the selected storage, installs under
`~/.local`, configures the user service for that selection and enables the daily
timer. Do not run the installer merely to change GUI preferences; the packaged
helper already supports recovery without scheduled permanent deletion.
The timer pins the selected XDG config/data locations. Installation refuses to
enable it if the caller's configuration directory differs from the user
manager's unit search location; align `XDG_CONFIG_HOME` with that user session
and retry. That failure may leave staged managed files and their backups, but
does not enable the timer. A failed GUI storage change restores the prior
selection.
