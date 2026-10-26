# Skills and MCP lifecycle

The source runtime exposes a fixed set of discovery and lifecycle tools through
both API requests and the native CLI room bridge. It does not put every room
skill summary or external MCP tool schema in the initial model context.

1. `find_capabilities` searches skill names/descriptions or configured MCP server
   names. Results are paginated, ten entries at a time. Selecting a server
   inspects only that server's tool summaries.
2. `use_skill` returns one selected playbook. `load_mcp_tool` returns one selected
   tool schema; its tool name comes from discovery.
3. `call_mcp_tool` invokes the loaded tool with its arguments. Loading a schema
   does not authorize execution: the existing exact-call approval still applies.
4. `unload_capability` releases a skill or tool. It can be discovered and loaded
   again whenever the task needs it.

Both load operations accept `lifetime`, defaulting to `task`:

| Lifetime | Release point |
| --- | --- |
| `task` | When none of the assignments held at load time remain claimed, in progress or blocked; checked before further model/tool work. If no assignment is held, the lease lasts for the current room turn. |
| `once`, skill | After the next successful model response has had the instructions available. |
| `once`, MCP tool | After one call attempt, including a refused or approval-pending attempt; or at the end of the room turn if unused. |

After an approval-pending one-off call, the agent reloads the tool and retries the
same arguments when the owner allows it. Approval remains tied to the actor,
tool, arguments, server configuration and room identity, and is consumed once.

MCP connections start only on demand and belong to one agent. Discovery closes
its temporary connection unless that agent already has a loaded tool on the
server. Unloading the last tool releases that agent's connection; another
agent's connection remains available. Unloading a stateful server ends its
session, so use a task lease while that session is needed. Workspace-transport
MCP servers retain their existing separate-process-per-exchange behavior.

Released skill bodies and tool schemas are replaced with small markers in
OrgBots-owned API history, preserving matching tool calls/results. Discovery
results are discarded from subsequent requests after consumption. Work results,
artifacts, source files, credentials and host configuration are not deleted.
Review-only agents may use skills but cannot discover, load or call external
MCP capabilities.

Native CLIs receive the same lifecycle gateway, but own their conversation
transcripts and may inject their own skills, rules and tool catalogs. OrgBots
cannot erase already-read text from those native transcripts. It does not reset
sessions or edit host profiles to simulate that. Unloading cannot refund tokens
already consumed; any effect on provider cache usage requires measurement.

This describes the source change. It does not imply that the installed desktop
package has been rebuilt or distributed.

## Verification on 26 October 2026

The affected runtime and server suites passed: 128 runtime tests and 104 server
tests. Fixtures exercise API request contents, native room-bridge dispatch,
one-use and task expiry, parallel discovery, reload after compaction, interruption
before checkpoint/restore, exact-call approval, independent agent connections,
and a slow server that must not block unrelated connections.

Two existing server tests require a private notification or legacy-journal
fixture and remained ignored. Opt-in live-provider checks were not enabled.
Two existing unused-variable warnings remain in runtime restart tests. Astra
review approved the implementation after the interruption, compaction and
same-response expiry regressions passed. No paid model calls or token-savings
benchmark were run for this change.
The optimized `orgbot-server` release build also passed.
