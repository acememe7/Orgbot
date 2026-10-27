# OpenCode and Antigravity integration

This describes the development source after 2.0.2. It does not change the
published package, installed desktop application, website results or earlier
benchmark claims.

The later [live-test report](benchmarks/2026-10-27-subscription-free-validation.md)
records an attempted protected OpenCode V4.1 Flash Free call. The provider
reported its free-usage limit reached; the CLI timed out without completed
usage. No paid fallback was attempted. The local mock results below remain
separate from that blocked live attempt.

| Provider | Executable | Model selection | Status |
| --- | --- | --- | --- |
| OpenCode | `opencode` | Explicit `provider/model` | Protected Linux adapter using an existing API credential; OAuth reuse blocked |
| Antigravity | `agy` | Explicit model slug, tested as `gemini-3.8-flash-medium` | Protected room-MCP smoke, 11-check context/restart test, and authorized MCP workspace write/readback passed |

`antigravity` on this machine launches the IDE and is not the CLI adapter.
Finding either executable does not establish account access. Sign-in starts
only through an explicit owner action. OrgBots does not substitute an API key
for a CLI subscription.

## Profile and tool boundaries

The new adapters require the protected Linux runtime. Each provider gets a
separate private home for each bot, preserving older provider sessions when a
bot changes provider. Existing Codex, Claude and Grok profile handling remains
unchanged. No host D-Bus socket is exposed.

OpenCode receives only the selected provider's existing API-key record from its
`auth.json`. The private copy is synchronized before each episode; removing the
source credential prevents reuse of an older copy.
Host credentials are not written back. External plugins, project configuration,
managed machine configuration and remote `wellknown` configuration are excluded.
Built-in plugins remain available, but OAuth records are rejected before launch:
refreshing a cloned OAuth token can invalidate the original account session.
No supported provider-specific refresh-sharing contract or live verification
establishes that this is safe here. OrgBots does not replace OAuth with API billing.
The private OpenCode home and configuration directories are read-only, with
writes limited to its data, state and cache directories. Generated settings
allow the OrgBots room MCP bridge; native tools are denied.
The room's existing workspace permissions and exact-call external-MCP approvals
still apply through that bridge.

In the initial implementation, Antigravity's native authentication used the
Linux keyring, and no narrowly selected credential transfer had been verified.
That adapter failed before invoking a model rather than forwarding the host
Secret Service or switching to Gemini API billing. The historical diagnosis
below records that state; the later private sign-in and guarded execution path
are described separately.

### Antigravity authentication diagnosis, 27 October 2026 (historical)

The following records the state before owner sign-in and protected runtime
integration. Later sections document the successful runtime smoke and the
remaining acceptance gates.

The installed `agy` 1.4.8 is present. OrgBots deliberately rejects it in the
provider constructor, private-profile builder and workspace launcher because
its saved desktop sign-in is not available inside that protected profile.
Current official authentication/help documentation provides no verified
personal-account token import/export interface. Gemini API-key and Cloud ADC
authentication are separate routes and were not substituted.

A separate D-Bus session and private Secret Service were tested with a synthetic
sentinel only. Storage, retrieval and retrieval after restarting the test
keyring daemon passed. A short private runtime directory was necessary because
the initial control-socket path was too long. No host keyring was queried or
copied.

The real installed CLI was then started headlessly in bubblewrap with network
disabled, `/run` masked, a cleared environment and that empty private profile.
It emitted an OAuth authorization-code prompt and waited until the 20-second
timeout. Current headless documentation describes an authentication-required
exit without a terminal; the installed binary behaved differently here.
No browser sign-in, provider completion or API/ADC fallback occurred.

At that point this established a possible private-keyring foundation, not
working AGY authentication. Subsequent owner sign-in and guarded execution are
described below. The
[sanitized diagnosis](benchmarks/2026-10-27-antigravity-auth-diagnosis.json)
records the exact tested binary and scope.

### Separate private sign-in in the development source

Run `orgbot agy login` in an owner terminal, or choose **Sign in** beside
Antigravity in **Connections & tools**. From this checkout, the equivalent is
`python3 app/packaging/orgbot-workspace.py agy-auth login`. Complete Google's
sign-in inside the terminal without submitting a model prompt. The helper stops
AGY when private credential items appear, closes the keyring and verifies that
the items remain visible after a locked restart. If credentials are already
stored, it returns immediately without launching AGY or reading the desktop
wrapping key.
`orgbot agy status` and **Check sign-in** report only whether the private keyring
contains credential items. They do not certify account access or runtime
readiness; account access is checked when the selected bot runs.

The helper creates a dedicated encrypted keyring under
`$XDG_DATA_HOME/orgbot/agy` (normally `~/.local/share/orgbot/agy`). A generated
wrapping passphrase is kept in one exactly scoped OrgBots item in the desktop
Secret Service. Only the trusted helper reads that item. It passes the
passphrase to the private keyring daemon through stdin, never through command
arguments, environment variables or a plaintext file. Missing, ambiguous or
unavailable wrapping keys fail closed; an existing encrypted store is not reset.
The existing host AGY login is neither read nor copied.

The private bus has no service activation and permits Secret Service traffic
and requested replies. AGY receives only its private socket. Its filesystem
starts empty, with read-only system runtime files and a private home/project.
The host home, desktop bus, encrypted keyring files and wrapping passphrase are
absent. The keyring daemon has its own separate home and data directories.
Controlled settings and their ancestor paths cannot be replaced; only CLI
data directories are writable. Global/project hooks, plugins and MCP servers
are excluded during sign-in. API/ADC variables are not inherited and
`useG1Credits` is explicitly false. That setting is not account-billing proof.

Only one AGY authentication operation may run at a time; a concurrent request
returns a busy error. Owned child processes stop on cancellation or parent
death. The helper requires an interactive terminal for login and does not
submit a model prompt. Status uses locked private-keyring metadata and does not
query the desktop keyring. The package declares its D-Bus and GNOME Keyring
dependencies, but no updated package has been installed or published.

The initial owner sign-in saved private credentials, then showed **Settings
Error**: AGY's interactive startup tried to normalize and atomically replace
the read-only settings file. The earlier networkless test reached OAuth only
and did not exercise that authenticated startup. The helper now ends at
credential storage instead of continuing into the full interactive session;
the protected settings remain read-only. Continuing with defaults or
overwriting the controlled policy remains prohibited. This is an enrollment
flow stop, not the current protected model-runtime path.

The owner's stored-credential path now returns successfully. Separately,
`agy -p /changelog --output-format json` completed inside the empty-root sandbox
using the existing private keyring with networking disabled and settings
unchanged. This proves that the saved credentials can be loaded for that
built-in command; it did not establish model execution or token refresh.

The next response test exposed a separate missing default-project record. The
helper now precreates the exact structural metadata written by AGY 1.4.8 and
keeps the whole global configuration read-only. Existing records are validated,
not overwritten. Two direct protected Gemini 3.8 Flash Medium probes then
returned `READY`; the final source-generated-profile probe reported 6,781 input
tokens, 30 output tokens (including 29 thinking tokens), and zero cache reads.
It confirmed model `gemini-3.8-flash-medium`, unchanged protected configuration,
and no stderr. No API credential was supplied and `useG1Credits` remained false.
These direct CLI checks did not validate the OrgBots room MCP/runtime adapter,
shared context or separate room-ledger usage. One guarded server MCP smoke is
now recorded below.

Cross-context restart, isolated-room behavior and room-ledger usage
reconciliation are now covered by the separate context proof below. The
[enrollment verification record](benchmarks/2026-10-27-antigravity-enrollment.json)
separates synthetic/offline proof from those remaining account tests.

These measures preserve the existing [workspace boundary](WORKSPACE_SAFETY.md).
The normal room CLI sandbox still exposes the host filesystem read-only, including
readable files in the host home; it is not complete credential secrecy from a
malicious CLI. Private profile copies isolate writable state. Both AGY enrollment
and protected runtime use empty-root filesystems, with only their specific
private home, runtime paths and private D-Bus socket mounted.

### Protected Antigravity room-MCP smoke, 27 October 2026

The owner completed private sign-in. The current protected Linux server path
passed a real AGY 1.4.8 room-MCP smoke using the explicit model
`gemini-3.8-flash-medium`. The model returned `READY`; the episode recorded
20,515 input tokens, 1,945 output tokens and zero cache tokens. This is one
successful guarded server execution, not broad context-sharing acceptance.

The runtime gives this bot a private `HOME`, uses the existing isolated
keyring wrapped by one exact host Secret Service item, and provides only the
private D-Bus socket. AGY's settings, global configuration and seeded default
project record are read-only. The private project `.agents` path and plugin
directory are empty. The room MCP bridge is the only allowed MCP server;
native command, file and URL tools are denied. No API credential or API
environment variable is supplied, and `useG1Credits` is false. The adapter
rejects a concurrent AGY session before making a provider call; only one
active AGY run is supported.

The account route is the stored native account sign-in. Provider billing
classification remains unknown: the smoke does not prove an invoice amount,
quota treatment or zero charge. The separate 11-check context/restart proof is
described below. See the
[sanitized runtime report](benchmarks/2026-10-27-antigravity-runtime.json).

### Context, restart and isolated-room proof

A second guarded run passed all eleven checks in
`context-2/proof.json`. Astra routed an exact nonce challenge to Gemini 3.8 Flash
Medium. After restart, Gemini resumed the same native session and recalled the
marker. A fresh room used a distinct native session and returned `UNKNOWN` for
the marker. A hostile `.agents` input remained masked, and its source checksum
was unchanged. Independent AGY transcript totals matched the engine usage and
persisted baseline; the restart episode was recorded as a delta once.

| CLI episode scope | Episodes | Input | Output | Cache read / write |
| --- | ---: | ---: | ---: | ---: |
| Main room, Astra `gpt-6-astra` via Codex | 1 | 13,100 | 246 | 61,184 / 0 |
| Main room, Gemini `gemini-3.8-flash-medium` via AGY | 2 | 53,013 | 5,010 | 29,868 / 0 |
| Isolated room, Gemini via AGY | 1 | 12,707 | 278 | 0 / 0 |

These counts are CLI episodes, not AGY internal model steps or invoice events.
The isolated room's usage is separate from the main-room totals. The usage
ledger reports AGY `billing_kind` as unknown and leaves actual API cost unset.

### Authorized workspace write and direct-write denial

A separate guarded run passed the room workspace's `workspace_write` policy:
the agent created `authorized.txt` through the room MCP tool and read back the
exact content. The original synthetic file remained unchanged. The project
mount remained read-only and native file tools retained their deny rules;
shell commands were disabled. The separate read-only `permissions-2` probe
attempted a native filesystem write and confirmed its denial. The authorized
write run tests the room-tool path without
granting the CLI native filesystem write access. Usage for the one CLI episode
was 53,980 input tokens, 844 output tokens, and zero cache tokens. The
provider's billing classification remains unknown.

Native schema-file reads remained blocked; room tool schemas were supplied in
the standing brief. Credential refresh and live approvals for external MCP
tools and shell-command workflows have not been tested.

For local reproduction, the protected relay artifact must be owner-controlled,
single-linked and not group- or world-writable. A Cargo artifact with hardlinks and mode
`0775` was correctly rejected; the test used a private checksum-identical copy
without installing it. AGY log compatibility permits only the `cli.log`
symlink to a relative `log/cli-YYYYMMDD_HHMMSS.log` target, which must resolve
to an owner-owned, single-link regular file in the private log directory. Logs
are retained. All other symlinks and path escapes are rejected.

## Sessions and usage

Each bot resumes only its saved native session, with provider/model/project/
policy checks. OpenCode sums distinct `step_finish` usage records per episode.
Antigravity's parser converts terminal cumulative usage to episode deltas using
the persisted baseline. Incomplete usage stays unknown. A missing terminal event
is an error, and a failed episode is not integrated into the owner project.

Input, output and cache usage remain separate. Reasoning is not counted twice.
CLI usage is not an invoice or subscription-quota measurement; estimated model
costs do not establish a provider charge. The existing serialized `codex_usage`
field remains compatible with previous source versions.

## Verification

The real OpenCode 1.19.4 executable was checked with a local OpenAI-compatible
mock and synthetic API credentials, using the production workspace helper and
a separate bubblewrap invocation:

- The production-helper fresh/resume baseline exited successfully with terminal
  `stop` events. Each reported 17 input and 4 output tokens, with zero cache
  tokens. That baseline made three requests, including title generation.
- A fixed title on fresh sessions prevents OpenCode's extra title-generation
  model call, which its episode usage events omitted. A separate bubblewrap run
  using the current Rust-generated profile verified the corrected fresh/resume
  arguments and exactly two model requests, with the same reported token counts.
- Through the production helper with the final title flag, the CLI discovered
  and called `orgbot_room_room_ping`; the mock model received its tool result.
  This proof made two requests. The final step reported 22 input and 7 output
  tokens.
- The effective configuration retained deny-by-default permissions and excluded
  hostile project and synthetic system MCP/configuration entries. The system
  configuration fixture existed only inside the test namespace.
- Private credential output contained only the selected provider's `type` and
  `key`. Host D-Bus and API-key environment variables were not inherited.

The [sanitized conformance record](benchmarks/2026-10-27-opencode-cli-conformance.json)
contains the commands, checksums, effective configuration checks and mock
request results. These OpenCode checks made **zero external model calls** and
prove CLI protocol and sandbox behavior against fixtures; they do not prove
real account authentication or provider billing. The separate Antigravity live
smoke is documented above.

Rendered Flutter widget fixtures with the app's Inter font were inspected at
375px and 1280px: provider choices, explicit-model fields, OpenCode's credential
limit, and Antigravity's separate sign-in and account-check-on-run status. These
are source UI fixtures, not captures of a rebuilt or installed desktop package.
Fifteen host configuration and rule files matched their pre-task SHA-256 hashes.

Final source suite totals and Astra review status are recorded in the
[project task log](LINUX_2_0_PLAN.md).

The isolated OpenCode audit binary is release **1.19.4**. Its release archive SHA-256 is
`849bd6b3d959a1d2ef200ed1634a51c79a45ca236aa26665d75debd694805cce`,
matching the official release asset digest. The extracted executable SHA-256 is
`538fcb1b6408c8d4608822acee19f7ff5b3a3eb3383c0633c4f06170857312a7`.
It was not installed into the
owner's CLI path. The installed `agy` reports **1.4.8**. Its `models` command
alone does not prove authenticated execution; the later guarded smoke above
provides the current limited runtime evidence.

## Upstream contracts checked

- [OpenCode 1.19.4 release](https://github.com/anomalyco/opencode/releases/tag/v1.19.4)
- [OpenCode run command](https://github.com/anomalyco/opencode/blob/v1.19.4/packages/opencode/src/cli/cmd/run.ts)
- [OpenCode configuration loading](https://github.com/anomalyco/opencode/blob/v1.19.4/packages/opencode/src/config/config.ts)
- [OpenCode built-in and external plugins](https://github.com/anomalyco/opencode/blob/v1.19.4/packages/opencode/src/plugin/index.ts)
- [Antigravity headless CLI](https://www.antigravity.google/docs/cli/headless/)
- [Antigravity authentication](https://www.antigravity.google/docs/install?tab=cli)
- [Antigravity settings](https://www.antigravity.google/docs/settings?tab=cli)
