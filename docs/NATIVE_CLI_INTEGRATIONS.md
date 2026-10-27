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
| Antigravity | `agy` | Explicit model slug | Separate private sign-in implemented in source; execution blocked pending live authentication and permission checks |

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

Antigravity's native authentication uses the Linux keyring. No narrowly selected
credential transfer has been verified on this machine. The adapter therefore
fails before invoking a model instead of forwarding the host Secret Service or
switching to Gemini API billing. No Antigravity process, project hook or plugin
is started by the adapter. Isolated authentication and executable configuration
boundaries remain integration requirements, not a working subscription connection.

### Antigravity authentication diagnosis, 27 October 2026

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

This establishes a possible private-keyring foundation, not working AGY
authentication. Application integration and an owner-completed sign-in into
that isolated profile are still required, followed by restart/refresh,
subscription/overage, configuration-boundary and live context/usage checks.
The production execution gates remain enabled. The
[sanitized diagnosis](benchmarks/2026-10-27-antigravity-auth-diagnosis.json)
records the exact tested binary and scope.

### Separate private sign-in in the development source

Run `orgbot agy login` in an owner terminal, or choose **Sign in** beside
Antigravity in **Connections & tools**. From this checkout, the equivalent is
`python3 app/packaging/orgbot-workspace.py agy-auth login`. Complete Google's
sign-in inside the terminal and exit AGY without submitting a model prompt.
`orgbot agy status` and **Check sign-in** report only whether the private keyring
contains credential items; they do not certify authentication or enable bots.

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

Owner sign-in, credential refresh, subscription/overage behavior, actual native
tool permissions, room MCP, and live context/usage checks remain required
before removing the three execution gates. The
[enrollment verification record](benchmarks/2026-10-27-antigravity-enrollment.json)
separates synthetic/offline proof from those remaining account tests.

These measures preserve the existing [workspace boundary](WORKSPACE_SAFETY.md).
The normal room CLI sandbox still exposes the host filesystem read-only, including
readable files in the host home; it is not complete credential secrecy from a
malicious CLI. Private profile copies isolate writable state. The separate AGY
enrollment sandbox described above uses an empty filesystem instead.

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
request results. There were **zero external model calls**. These checks prove
CLI protocol and sandbox behavior against fixtures; they do not prove real
account authentication, provider billing, paid inference or a completed live
OrgBots team run with these new providers.

Rendered Flutter widget fixtures with the app's Inter font were inspected at
375px and 1280px: provider choices, explicit-model fields, OpenCode's credential
limit and Antigravity's unavailable status. These are source UI fixtures, not
captures of a rebuilt or installed desktop package. Fifteen host configuration
and rule files matched their pre-task SHA-256 hashes.

Final source suite totals and Astra review status are recorded in the
[project task log](LINUX_2_0_PLAN.md).

The isolated OpenCode audit binary is release **1.19.4**. Its release archive SHA-256 is
`849bd6b3d959a1d2ef200ed1634a51c79a45ca236aa26665d75debd694805cce`,
matching the official release asset digest. The extracted executable SHA-256 is
`538fcb1b6408c8d4608822acee19f7ff5b3a3eb3383c0633c4f06170857312a7`.
It was not installed into the
owner's CLI path. The installed `agy` reports **1.4.8**. Its `models` command
succeeded, but that alone does not prove authenticated model execution.

## Upstream contracts checked

- [OpenCode 1.19.4 release](https://github.com/anomalyco/opencode/releases/tag/v1.19.4)
- [OpenCode run command](https://github.com/anomalyco/opencode/blob/v1.19.4/packages/opencode/src/cli/cmd/run.ts)
- [OpenCode configuration loading](https://github.com/anomalyco/opencode/blob/v1.19.4/packages/opencode/src/config/config.ts)
- [OpenCode built-in and external plugins](https://github.com/anomalyco/opencode/blob/v1.19.4/packages/opencode/src/plugin/index.ts)
- [Antigravity headless CLI](https://www.antigravity.google/docs/cli/headless/)
- [Antigravity authentication](https://www.antigravity.google/docs/install?tab=cli)
- [Antigravity settings](https://www.antigravity.google/docs/settings?tab=cli)
