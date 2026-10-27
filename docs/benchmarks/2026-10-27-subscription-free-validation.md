# Subscription and free-model live validation

Development-source tests on 27 October 2026. These results do not update the
installed application, published 2.0.2 package, GitHub or website. The owner
required testing before installation or publication.

## Selected routes

| Model | Route | Result |
| --- | --- | --- |
| GPT-6 Astra | Existing Codex ChatGPT sign-in, no OpenAI API key in its private profile | Coding, context, restart and room-isolation checks passed |
| DeepSeek Flash | Explicit DeepSeek API credential; `deepseek-flash` | Coding fixture and 16-check lifecycle passed |
| DeepSeek V4.1 Flash Free | Selected OpenCode API credential; `opencode/deepseek-v4.1-flash-free` | Provider reported free-usage limit reached; no completed inference |
| Gemini 3.8 Flash | Antigravity subscription route | Pending safe isolated authentication, as requested; no API fallback |

The owner selected V4.1 after the installed catalog did not list V4.2. An
OpenCode API key authenticates the selected route; its presence does not make a
free model paid. Conversely, a CLI name alone does not establish subscription
billing. Desktop routing keeps the explicitly selected CLI; errors do not
silently select a paid API provider. The allowance dialog now explains that
CLI caps count episodes, and actual charges depend on the selected account.

The [OpenCode catalog](https://dev.opencode.ai/v2/docs/console/models) describes
V4.1 Flash Free as temporarily free. This is a provider offering, not proof of
an invoice or remaining subscription allowance. DeepSeek's
[pricing documentation](https://api-docs.deepseek.com/quick_start/pricing/)
maps `deepseek-flash` to DeepSeek-V4.1-Flash-1017.

## Completed coding fixture

The existing two-module Python fixture ran with DeepSeek implementing both
tasks and Astra independently reviewing them through the protected Codex CLI.
Both tasks reached `submitted`, the room completed, and a separate verifier
passed all **six original tests unchanged**. Test-file hashes match the
original fixture. Runtime: **239.341 seconds**. This is one accepted team run,
not a baseline/team savings comparison.

| Participant | Completed calls/episodes | Uncached input | Cached input | Output | Configured API-cost estimate |
| --- | ---: | ---: | ---: | ---: | ---: |
| DeepSeek API | 17 API calls | 25,319 | 171,264 | 7,354 | $0.017458 |
| Astra / Codex | 1 CLI episode | 48,558 | 416,256 | 4,801 | Unknown; subscription route |
| Room sum | Different request units; do not combine them | 73,877 | 587,520 | 12,155 | No combined invoice claim |

Cache writes were zero. Provider/agent totals reconcile with the room's
persisted `UsageRecorded` events. A CLI episode can contain several internal
model requests; its count is not directly comparable with API request count.
The private Codex profile reported `auth_mode=chatgpt` and no OpenAI API key.
Subscription token usage is not an extra API bill, quota percentage or saving.

## Lifecycle and context checks

The first DeepSeek lifecycle attempt failed: the model did not record the
one-time skill nonce and retried an approved MCP call before reloading its
released schema. The second attempt used clearer ordering instructions and
passed the same **16 strict checks**: one-use skill/schema release, exact tool
arguments and approval, one actual fixture invocation, pending approval
persisted across restart, unchanged source sentinel and handoff to review.
The validator was not relaxed.

| Lifecycle attempt | Acceptance | API calls recorded | Uncached input | Cached input | Output | Configured cost estimate |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Failed | 13 | 5,130 | 79,104 | 2,495 | $0.005015 |
| 2 | Passed 16 checks | 14 | 10,714 | 80,896 | 2,490 | $0.006694 |

The final DeepSeek/Astra context attempt passed **nine applicable checks**:
directed marker exchange, one-use skill release, complete positive recorded
usage for both agents, recall after restart, unchanged native Astra session,
usage growth without regression, an isolated room returning `UNKNOWN`, absence
of the original marker in that checkpoint, and a distinct isolated native
session. OpenCode was explicitly excluded, so these results do not establish
the requested three-model exchange. The pre-restart usage/session-hash baseline
and both paused final checkpoints were retained.

Context attempt 1 failed its owner-target assertion: a test prompt mentioned
both participants and woke both. Prompts now mention only the intended target.
The harness also deduplicates replayed event sequences and waits for all
participants to settle before capture. Context attempt 2 was terminated by
SIGTERM before final proof. Retained checkpoints support the exchange and
isolated response, but do not establish the pre-restart session hash or final
capture; that attempt remains partial. Attempt 3 was stopped after identifying
the wait bug: the native CLI returned `UNKNOWN` with a trailing newline. The
harness now accepts surrounding framing whitespace while rejecting substantive
suffixes; primary marker checks remain exact. It also records interruption
failures and cleans up its owned server group. Attempt 4 passed after those
corrections. All attempts' reported usage is retained.

The OpenCode preflight used the actual Rust `AgentCliProvider` and
`WorkspaceGuard`, read-only workspace access, no MCP tools and the exact
selected free model. Its private log reported `Free usage limit reached`.
The CLI stayed active until the 180-second adapter timeout. Its empty native
usage record is **not evidence of zero consumption**. There was one attempt,
no paid fallback and no further free-model retries.

## Accounting boundaries

The dedicated DeepSeek allowance ledger has a **$0.25** conservative peak-rate
cap and an 80-request ceiling. CLI providers have 12-episode ceilings and
240-second turn timeouts. These are test limits, not changes to owner settings.
The OpenCode preflight had its separate one-attempt/180-second limit.

The allowance ledger records reservations before requests. Completed responses
replace reservations with reported token/rate estimates; cancelled requests
can retain their conservative reservations. After the two lifecycle attempts,
the ledger held 29 requests and $0.036772 of budget commitment, whereas
checkpoint events reported 27 completed calls and $0.011709 of estimates.
The two additional reservations cannot be attributed precisely from the
available aggregate ledger. They were retained; the ledger was never reset.

`unreported_requests=0` does not prove all reservations settled: dropped futures
can bypass settlement. Tracking durable reservation identities/statuses is a
remaining telemetry limitation. Reported event completeness, budget commitment
and actual billed consumption are distinct. There is no invoice reconciliation,
full interrupted-call token total, or generalized cost-saving claim here.

Across all retained attempts, completed events reported **60 DeepSeek API
calls** (62,842 uncached input, 407,040 cached input, 13,538 output) and **12 Codex
CLI episodes** (147,011 uncached input, 934,656 cached input, 7,384 output).
Codex episode count and token totals match its allowance ledger exactly.
DeepSeek's reported peak-rate estimate totals **$0.037572**; the conservative
ledger commitment is **$0.062635**, including **$0.025063** retained for the two
unsettled reservations. These amounts are within the $0.25 test cap and are not
reconciled invoices. OpenCode's unreported preflight is not added as zero usage.

## Evidence and checks

Runtime engine: source commit `2e3eaab`, built locally, SHA-256
`29aaf3eada732fb5640ac9e843ad4ebef648e1da31b92e7f8a2f06107f3358e8`.
OpenCode 1.19.4 audit binary SHA-256:
`538fcb1b6408c8d4608822acee19f7ff5b3a3eb3383c0633c4f06170857312a7`.
The audit binary was not installed into the owner's CLI path.

The [sanitized record](2026-10-27-subscription-free-validation.json) contains
per-attempt results and reported usage without credentials, raw prompts,
session IDs or private account balances. Raw synthetic checkpoints and logs
remain in an owner-only evidence directory.

All 48 benchmark/probe unit tests and four allowance widget tests passed.
Targeted Flutter analysis reported no issues.
The allowance notice was rendered and inspected at 375 px and 1280 px; it is
legible, its controls remain visible, and no overflow was reported. Runtime
Rust source did not change in this test milestone. Broader prior regression
results are recorded in the project task log, not rerun or relabeled here.
All 16 recorded host configuration/rule/auth files retained their original
hashes; the private profiles did not rewrite those owner files.
Final Astra review approved this scoped source/evidence milestone after
independently checking checkpoint hashes, usage sums, lifecycle validation,
restart baseline, paused checkpoints, both allowance renders and the 48 tests.

OpenCode live completion, its context/usage/restart tests and safe subscription
OAuth reuse remain open. Gemini/Antigravity execution remains pending. No
package installation, publication or deployment is included.
