# OpenCode and Antigravity integration

This describes the development source after 2.0.2. It does not change the
published package, installed desktop application, website results or earlier
benchmark claims.

| Provider | Executable | Model selection | Status |
| --- | --- | --- | --- |
| OpenCode | `opencode` | Explicit `provider/model` | Protected Linux adapter using an existing API credential; OAuth reuse blocked |
| Antigravity | `agy` | Explicit model slug | Configuration and stream adapter implemented; execution blocked pending isolated authentication |

`antigravity` on this machine launches the IDE and is not the CLI adapter.
Finding either executable does not establish account access. The app does not
start a login, connect another account, or substitute an API key for a CLI
subscription.

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

These measures preserve the existing [workspace boundary](WORKSPACE_SAFETY.md).
The normal CLI sandbox still exposes the host filesystem read-only, including
readable files in the host home; it is not complete credential secrecy from a
malicious CLI. Private profile copies isolate writable state.

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
