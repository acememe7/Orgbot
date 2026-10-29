# Linux agent workspace boundary

`app/packaging/orgbot-workspace.py` gives each agent a private working copy of
one project. The project source is never mounted writable inside a model CLI.
The helper stores its baseline manifest outside the agent worktree and mounts
that host root read-only through bubblewrap. A missing bubblewrap executable or
failed sandbox setup stops the run.

The helper speaks newline-free JSON on standard output. Errors use
`{"ok":false,"error":"..."}` and a non-zero exit code. Standard error from
the child process remains the child’s standard error.

```text
orgbot-workspace.py prepare --project ABS --state ABS --agent ID [--read-only]
orgbot-workspace.py status --state ABS --agent ID
orgbot-workspace.py run --state ABS --agent ID [--read-only] [--verify] [--private-home ABS] [--profile codex=ABS] [--profile claude=ABS] [--profile grok=ABS] [--agent-cli opencode|agy] [--mcp-socket ABS] -- PROGRAM ARG...
orgbot-workspace.py apply --state ABS --agent ID [--review-token SHA256]
```

`prepare` returns the host paths `workspace`, `profile_root`, and
`private_home`, plus `private_home_mount`, `mcp_socket_mount`, `project_id`,
and `read_only`. The mount paths are exact in-sandbox destinations and should
be used when writing CLI or room-MCP configuration. It copies the project into
`workspace` and creates `private_home` under the agent’s private profile root.
The trusted launcher may copy the owner’s CLI configuration and authentication
files into `private_home`; `HOME` inside the sandbox points to a private mount
under `/run/user/<uid>/orgbot-home`. The host home remains visible through the
read-only root so installed CLI binaries resolve, with known secrets hidden:
SSH and GPG keys, every CLI's sign-in file, cloud, registry and Git
credentials, browser profiles, shell histories, and OrgBots' own data, which
holds every room's access token. A folder is covered with an empty tmpfs where
no program lives in it; where a CLI keeps its program beside its sign-in (Codex,
Grok), only the sign-in file is covered and reads as permission denied. This is
a list of known locations (`_OWNER_SECRET_DIRS` and `_OWNER_SECRET_FILES`), not
a full boundary: other files in the home, such as a project's `.env`, stay
readable.

Bots reach servers over SSH only through `~/.local/share/orgbot/bot-ssh`
(under `$XDG_DATA_HOME` when set), and only with the owner's permission for one
task. A bot holding a task that needs a server calls `request_server_access`
with what it will do; the owner sees a decision and must confirm "Allow server
access for this task". From then on, for that bot and that task only, the
folder is mounted read-only as the account's `~/.ssh`, which is where OpenSSH
looks whatever `HOME` says, provided the folder is owned by the user and
private (mode 0700). The room decides this afresh for every turn and every
command: the key is not mounted while the task is blocked, waiting on
unfinished work, on hold, in review or done, never for a review-only or
read-only bot, and never for any other bot. Every other sandbox sees an empty
`~/.ssh`, and the folder is hidden at its own path too. The folder holds only
the keys, host entries and known hosts meant for bots, so `ssh <host>` works for
those servers and no others. Revoke a server by deleting the bot key from its
`authorized_keys`. A sandbox started with the key keeps it until that turn
ends, so a hold placed mid-turn applies from the next turn; restrict the bot
key in `authorized_keys` (for example `restrict,from=...`) for a server-side
limit as well.
Optional `--profile` binds accept only that agent’s private
`private_home/.codex`, `private_home/.claude`, or `private_home/.grok` directory.

Each run sees the project copy at `/tmp/orgbot-workspace` and as its current
directory; `ORGBOT_WORKSPACE` names the same in-sandbox path. The helper clears
the inherited environment, then sets a filtered absolute `PATH`, private
`HOME`, `TMPDIR`,
locale, optional safe `TERM`, `ORGBOT_WORKSPACE`, and optional room MCP socket
path. The private CLI home stays writable for auth/session refresh, even for
reviewer runs; reviewer project files are read-only. In particular, SSH agent
and D-Bus variables are not inherited. The sandbox has a read-only host root,
new PID/IPC/UTS namespaces, a private `/run` tmpfs (and `/var/run` when it is a
real directory), a private 1 GiB `/tmp`, and a
1 GiB per-file write limit. Its writable locations are the worktree, private
CLI home under `/run/user/<uid>`, and bounded `/tmp`. The helper rejects project
copies above 150,000 files, 20 GiB total, or 512 MiB per file. A
passed Unix room-MCP socket is mounted into that private runtime tree. Other
Unix sockets under host-visible paths outside `/run` (including a socket stored
in a readable home directory or project) are not automatically hidden; this is
not a general Unix-socket filtering boundary.

The helper also points native CLIs at their private profile directories with
`CODEX_HOME`, `CLAUDE_CONFIG_DIR`, and `GROK_HOME`. Those directories remain
writable for provider session state. Host CLI binaries and their installation
files are readable through the read-only host root. Files in the host home
outside the hidden list remain readable; the launcher’s copied profiles provide
writable session isolation, and the hidden list covers the known credential
locations, not every secret a home might hold.

The development OpenCode adapter uses a separate `profiles/home-opencode`
directory and a generated, read-only configuration. It imports only the selected
provider's existing API-key credential, excludes remote `wellknown`
configuration, and masks `/etc/opencode` configuration. Project configuration
and external plugin autoloading are disabled. Its private home is read-only,
with writable data, state and cache directories. OAuth credential reuse is
blocked. `--agent-cli agy` fails before starting a child process because
isolated Antigravity authentication has not been verified. See
[native CLI integration status](NATIVE_CLI_INTEGRATIONS.md).

## Independent reviewer tests

Protected local rooms with command execution enabled expose `workspace_verify`
to reviewers as well as workers. This uses a separate read-only project copy
and never integrates changes. `workspace_run` remains unavailable to review-only
agents. The verifier starts with an empty filesystem root, read-only system
runtimes, private bounded temporary/home directories, a cleared environment and
networking disabled. Host homes, CLI profiles, room state and MCP sockets are
not mounted. The helper's `--verify` mode rejects profile and socket options.

Tests must use installed system runtimes and direct generated output to `/tmp`.
Python checks can use `python3 -B -m unittest -v`. Tools installed only in a
user's home, dependency downloads and ordinary in-tree build output are not
available in this mode. A check that needs them must be reported as unavailable;
passing the Python fixture does not establish that every project's build can
run in the sandbox. Android and SSH workspaces do not advertise this Linux tool.

## Integration and recovery

`status` returns `changed` records (`path`, `kind`), `added`, `deleted`, and
`conflicts` path lists, plus a `review_token`. The token is a SHA-256 digest of
the canonical project, agent, baseline, owner-project, and agent-workspace
snapshot. A
conflict means both the owner’s project and the agent’s copy changed the same
path to different contents. `prepare` refreshes owner changes into clean agent
files and preserves agent-only changes, even if another path conflicts.
Conflicting files remain untouched in both trees and are reported until a human
or trusted integration resolves them.

`apply` performs a three-way comparison of the trusted baseline, current
project, and agent worktree before writing anything. If any conflict exists,
the whole batch is rejected before mutation. New and changed files are written
through same-directory temporary files and atomic links/replacements. Existing
project bytes are moved through the installed trusted `orgbot-trash` helper
before replacement. The helper receives the reviewed file SHA-256 and verifies
it before moving the source; a changed or replaced file is retained. A target
that was absent during conflict checking is never moved to trash: if it appears
before integration, the operation fails and preserves it. If the trusted trash
helper is missing, cannot verify its selected storage, or rejects the path, the
workspace helper stops without replacing existing bytes. An
out-of-tree journal records each planned operation and its progress so an
interrupted application leaves preserved originals recoverable.
`apply` must be invoked by the trusted host integration after the sandboxed
process exits; project and state paths are read-only from inside `run`.

On Debian installs, `orgbot-trash` is packaged at `/usr/bin/orgbot-trash` with
its implementation at `/usr/lib/orgbot/orgbot-trash.py`. From 2.0.1, new users
default to their local data directory; **Connections & tools → File recovery** selects a
different private recovery folder. All helper operations share the same
owner-private configuration. Existing HDD installations retain
`/mnt/hdd/OrgBots Trash` and its ext4 mount requirement; ambiguous package-only
upgrades require an explicit selection. A configured filesystem that is absent,
read-only, replaced or unverifiable causes the helper to stop without fallback.
Changing roots never moves or purges existing entries. See
[recoverable trash](../app/linux/TRASH.md) for selection and retention details.

Agent deletions are listed as pending and remain unapplied unless the trusted
owner flow passes the exact `review_token` returned by `status` after the owner
reviews and approves that deletion list. The helper recomputes the token under
the project integration lock before any mutation. A stale token fails the whole
batch and returns the current paths and token for renewed review; there is no
blanket `--allow-delete` option. Without a token, clean non-deletion changes
may apply while deletions remain pending. The helper itself does not infer
approval from model output.

Workspace copies preserve relative symlinks only when their resolved targets
stay inside the project and are included in the copy. Absolute, dangling,
external, and generated-tree symlinks fail closed. Special files,
multiply-linked files, unsafe path components, oversized files, and trees
beyond the configured entry and byte limits are rejected. The helper omits
rebuildable directories such as `node_modules`, `.dart_tool`, `target`, `build`,
`dist`, virtual environments, and common tool caches. These folders are not
copied or merged; rebuild them inside the agent workspace as needed. `.git` is
copied once so local Git commands can inspect the project, but is excluded from
status and apply. Empty directories remain staged in the private workspace;
file writes create canonical parent directories when integrated. State
directories and manifests must be private, owner-controlled paths. Baseline
manifests are validated on every operation; malformed or missing metadata
fails closed. Project preparation and integration are serialized with an
owner-private per-project `flock`; the lock is released before the CLI runs so
room-MCP workspace calls cannot deadlock.

The sandbox preserves network access so native provider CLIs can reach their
configured services. Remote MCP servers and other services reachable over the
network are outside this local filesystem boundary; their authorization and
side effects require separate controls. This helper does not claim to confine
remote tools or every kernel attack surface.

On Linux, OrgBots routes configured external MCP tools through the room bridge.
Each call pauses for an **Allow once / Deny** owner question showing the server,
tool and arguments. Approval binds the exact agent, arguments, configuration and
room; its use is durably recorded before dispatch. Private CLI profiles inherit
only the room bridge, leaving original host configurations untouched. Review-only
agents cannot invoke external MCP tools. Approving a remote tool permits its
external effects; those effects do not pass through the local trash helper.

Run the fixture suite with:

```sh
python3 app/packaging/test-orgbot-workspace.py
```

The sandbox tests execute the installed bubblewrap binary. They use disposable
temporary projects and a test-only replacement for the trash preservation
function; no production trash-mount bypass is exposed by the helper CLI.
