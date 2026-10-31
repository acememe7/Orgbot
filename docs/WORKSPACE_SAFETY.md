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
credentials, deploy and hosting tools' sign-ins (Wrangler, `cloudflared`,
Vercel, Netlify, Fly.io, DigitalOcean, Supabase, Stripe, Railway, and the
`configstore` folder Firebase uses), browser profiles, shell histories, the app's
own support folder (tool approvals and the allowance ledger), and OrgBots' own
data, which holds every room's access token, also where `XDG_DATA_HOME` puts it
outside the home's `.local/share`. A folder is covered with an empty tmpfs where
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
A workspace that was ever read-only never gets the key, even when the room-tool
path does not repeat `--read-only`.

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

## Access modes and lent sign-ins

Each room has an access mode that decides which requests for the owner's
things are approved on their own and which ask the owner. Requests are rated
regular, risky or critical:

| Mode | Regular | Risky | Critical |
|---|---|---|---|
| Approval (default) | asks | asks | asks |
| Half approval | approved | asks | asks |
| Auto | approved | approved | asks |
| Full write | approved | approved | approved |

Questions about the work itself (`ask_owner`) always reach the owner, in every
mode. Full write still confines bots to their own project copy.

A bot asks with `request_access`: the task, what it will use (`server` for the
bot server key, or the name of a lent sign-in), what it will do (`inspect`,
`change`, `release`, `destroy` or `spend`) and why. The request is rated the
higher of the action's rating (inspect and change are regular, release is
risky, destroy and spend are critical) and the risk the owner gave each thing
asked for (the server key is risky). When the mode approves the rating, the
request is recorded as a decision answered on the spot, with a note saying so;
otherwise it is asked like any question and the task waits on it. Either way
the grant is the answered decision, so a restored room grants exactly what was
allowed. `request_server_access` remains and asks for `server`.

Outside tool calls through MCP servers count as risky: Auto and Full write let
them through, Approval and Half approval ask for each call.

The owner lists the sign-ins they will lend in **Connections & tools → Sign-ins
bots may use** (saved in `$XDG_DATA_HOME/orgbot/lending.json`, which bots cannot
read). Every tool there is off until turned on. A room passes the enabled ones
to its engine when it starts, as `[[room.lend]]` tables in its room file. Every
listed path is hidden from every sandbox where it lives. A sign-in lent to a bot
for its task is mounted read-write into that bot's private home at the same
relative path, so the tool finds it under `HOME` and a renewed token reaches the
owner's copy. The same terms as the server key apply: only that bot, only while
the task is under way, never a read-only or review-only turn. The room service
and the launcher both refuse paths outside the home, through links out of it,
or into what is never lent: OrgBots' data, `~/.ssh`, `~/.gnupg`, and the model
CLIs' sign-ins, which bots get their own copies of. Lending is not available to
OpenCode or Antigravity bots, whose private homes are built differently. A lent
sign-in is usable over the network from the sandbox, so lend tokens scoped to
what the work needs where the provider allows it.

The GitHub CLI usually keeps its token in the desktop keyring, not in
`~/.config/gh`, and a sandbox cannot reach the keyring. When `~/.config/gh` is
lent and its `hosts.yml` holds no token, the launcher asks the owner's `gh auth
token` outside the sandbox and gives that turn `GH_TOKEN`, with git's GitHub
credential helper set to `gh auth git-credential`. The values reach bubblewrap
through a pipe (`--args`), never a command line or a file, and the keyring is
never opened to the bot. With no keyring sign-in the bot is signed out and keeps
its turn.

## Network inside agent CLIs

Codex runs its own sandbox inside OrgBots' one. In workspace-write mode that
sandbox refuses every socket, so a Codex bot could not start the local server a
browser test needs, though bots on other CLIs and its own room commands had the
network. A Codex bot allowed to write now runs with
`sandbox_workspace_write.network_access=true`; Codex still refuses writes outside
the project, and read-only Codex bots are unchanged. OrgBots does not confine the
network for any bot (see Known limits in the release notes).

## Landing a bot's commits

A bot's copy keeps its own git history, so commits made there never reach the
project by themselves. `workspace_land_commits` lands them: the bot writes a git
bundle inside the project and names it with the branch, the task it holds and
why. The room service copies the bundle out of the project by descriptor
(refusing links), checks it with git's hooks, global configuration and filters
off, fetches it into a temporary ref, and requires that its commits build on
where the branch stands now. When the branch is checked out, every file the
commits change must already be in the project byte for byte as they leave it,
so the project never seems to undo them. The owner then sees the commits on one
card, with a second tap to confirm; the room's access mode applies, rated
risky. The grant covers only those commits onto that branch from where it stood.
On the bot's next call the service re-checks the bundle's SHA-256, the branch
and the commits against the approval, moves the branch forward with
`git update-ref` (expecting the old value), and lets the index follow with
`git read-tree -m -i`, which keeps anything staged; if the index cannot follow,
the branch is put back. Nothing is pushed or merged, no history is rewritten,
and no project file is written. Review-only bots cannot land commits.

A landing card can name the checks recorded for those commits: an artifact on
the task, given as `checks`. The card then says which artifact it is, or says
that no checks were recorded.

## Pushing the owner's branch

`workspace_push_commits` pushes a branch of the owner's project that already
exists on the remote, such as `origin/main`, with the owner's own git setup.
The room service reads where the remote branch stands with `git ls-remote`,
refuses a branch the remote does not have, history that does not build on the
remote's commit, and more than 50 commits, and lists the commits on a card.
Every push asks the owner, whatever the room's access mode: Full write does not
skip it. After the owner approves, the service re-checks that neither the
branch nor the remote moved, then runs `git push <remote> <commit>:refs/heads/<branch>`
with hooks off and no terminal prompt. It never forces, deletes or creates a
remote branch.

## Copies that keep up with the project

`prepare` brings a copy's history along with its files. When the project's
commit has moved, the copy receives the project's missing object files. A copy
with no commits of its own has its branch moved to the project's commit and its
index rebuilt to match, so `git status` there shows only the bot's own changes.
A copy with commits of its own keeps them; the project's commit is written to
`project/<branch>` for the bot to rebase onto. Git is never run inside a copy,
because a bot can change that copy's git config: refs, objects and the index are
written as files, through directory descriptors that refuse symlinks, and the
index is built by git in the owner's project with hooks, monitors and user
configuration off. A copy with refresh conflicts is not touched.

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

Verify copies share their large files. Nothing but the helper changes a verify
copy, and it always replaces a file rather than writing into it, so each file
of 64 KiB or more is hard-linked to the identical file in another verify copy
of the same project, after that file's contents hash to what this copy's
baseline records. Refreshing one verify copy gives it new files and leaves the
others alone. Because nothing else writes to them, refreshes trust a verify
copy's baseline rather than re-reading every file. Copies a bot can write are
never shared: this filesystem has no copy-on-write, and a bot writing into a
shared file would change every copy at once.

## Leftover copies

Each bot can have two copies of the project: the one its own coding CLI works
in, named after its handle, and the one OrgBots uses for the room's workspace
tools, named after its id (plus a read-only `-verify` copy for reviewers). When
a room starts, `prune` removes copies belonging to bots no longer in that room,
but only when everything in the copy is already in the project or never left
it. A copy with changes that never merged (including deletions still waiting
for approval), a missing or unreadable baseline, or a project that is gone is
kept, and the room says which copies it kept and why. Current bots' copies and
the project itself are never touched. `prune --dry-run` reports what would be
removed without removing it.

The app lists every leftover copy under Workspace changes, Leftover copies, with
its size and why it was kept. `discard --agent NAME` removes one the owner picks:
first every file that is missing from the project or differs from it is copied
to `~/.local/share/orgbot/leftovers/<name>-<time>/`, and the copy's whole git
history too when it holds commits the project does not have. The copy's refs are
read as files and only the owner's project is asked about them. A copy that
belongs to a bot in the room is refused. A size counts only the bytes removing
the copy would free, so files shared with another copy are left out.

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

A bot's own merges (after a successful room command, a room file write, or a
CLI turn) use `apply --set-aside-conflicts`: the project's newer version is
never overwritten; the bot's version of each conflicting file is copied into its
private home under `~/orgbot-conflicts/<time>/`, its copy takes the project's
version, and every other change merges as usual. The result's `note` names the
files set aside, and the bot is told to redo its change on top. The folder is
created fresh and refuses links, so a bot cannot steer the copy elsewhere. A
command's output always reaches the bot: a merge problem is reported beside it,
and in a CLI turn it is reported in the turn rather than failing it. An owner's
deletion approval never sets files aside, because it covers the exact list the
owner saw.

An approved review moves removals to the trash in batches of 200, through one
call of the trash helper per batch, and every target is checked against the
reviewed list before the first one moves. The helper prints a progress line on
standard error for each batch and for the files applied, and the app shows
them as counts. A batch that fails part way stops the apply and reports what
already moved, with a receipt for each file.

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
