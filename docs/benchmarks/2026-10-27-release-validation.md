# OrgBots 2.0.2 release validation and usage reconciliation

Date: 2026-10-27

## Accepted live comparison

One bounded baseline/team pair completed and passed the unchanged fixture tests
in trial 3. Room completion, independent acceptance, and usage completeness are
recorded for both arms in the trial 3 `report.json`. This is one small accepted
pair; it does not establish general cost, quota, or speed savings.

The historical trial 3 room report sums cumulative Codex session totals once per resumed CLI
episode. Reconcile it using the final native `total_token_usage` record per
session instead. Native `input_tokens` includes cached input, so uncached input
below is `input_tokens - cached_input_tokens`. The counts are disjoint; cache
writes were zero. Native `token_count` event counts are transcript events, not
provider request counts.

| Arm | Model | Native token_count events | Uncached input | Cached input | Output | Cache write |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Baseline | gpt-6.1-sol | 17 | 60,388 | 921,600 | 10,975 | 0 |
| Baseline | gpt-6-astra | 20 | 61,933 | 1,107,840 | 6,749 | 0 |
| **Baseline total** | Codex | **37** | **122,321** | **2,029,440** | **17,724** | **0** |
| Team | gpt-6.1-sol | 12 | 32,223 | 420,608 | 4,744 | 0 |
| Team | gpt-6-astra | 17 | 56,932 | 909,824 | 9,257 | 0 |
| **Team total** | Codex | **29** | **89,155** | **1,330,432** | **14,001** | **0** |

Evidence files remain in the owner-private archive. Native filenames, with
session UUIDs redacted, are `rollout-2026-10-27T09-19-56-<redacted>.jsonl`
(baseline Sol), `rollout-2026-10-27T09-22-16-<redacted>.jsonl` (baseline
Astra), `rollout-2026-10-27T09-30-14-<redacted>.jsonl` (team Sol), and
`rollout-2026-10-27T09-32-33-<redacted>.jsonl` (team Astra). The engine episode
totals are in the trial 3 `report.json`; per-arm checkpoints are named
`room-state.json`. No prompt, reasoning text, private path, session identifier,
nonce, profile, or credential is reproduced.

| Arm and model | Engine episode sum: uncached / cached / output | Native final total: uncached / cached / output |
| --- | ---: | ---: |
| Baseline Sol | 100,055 / 1,261,440 / 16,229 | 60,388 / 921,600 / 10,975 |
| Baseline Astra | 110,093 / 1,632,128 / 11,067 | 61,933 / 1,107,840 / 6,749 |
| Team Sol | 63,420 / 794,880 / 9,454 | 32,223 / 420,608 / 4,744 |
| Team Astra | 149,215 / 1,707,776 / 19,662 | 56,932 / 909,824 / 9,257 |

Each engine row includes earlier cumulative snapshots again. For example,
baseline Sol's engine row is the sum of the first and final native snapshots;
the final native cumulative value is the session total. Use the native totals
above for this pair. The 2.0.2 episode-delta correction was subsequently verified
with three real native episodes, as recorded below.

The team also used DeepSeek Flash: 20,205 uncached input, 279,040 cached input,
13,164 output, zero cache write, and 22 API requests. The engine recorded a
$0.023542 API-cost estimate. This is not an invoice. Codex is subscription
billed, so its token count does not establish quota savings or an API price.

## Other paid attempts

These are separate from the accepted pair and are not comparison results.

| Trial | Arm/model | Result | Native final or API usage: uncached / cached / output | Token events or API calls | API-cost estimate |
| --- | --- | --- | ---: | ---: | ---: |
| 2 | Baseline Sol | Completed and accepted; pair incomplete | 76,697 / 1,269,888 / 9,879 | 23 token events | subscription |
| 2 | Baseline Astra | Completed and accepted; pair incomplete | 63,827 / 662,400 / 5,306 | 13 token events | subscription |
| 2 | Team Sol | Failed arm | 70,692 / 827,136 / 3,515 | 16 token events | subscription |
| 2 | Team Astra | Failed arm | 58,491 / 926,976 / 6,404 | 17 token events | subscription |
| 2 | Team DeepSeek Flash | Room remained running; not accepted | 20,992 / 172,544 / 6,186 | 19 API calls | $0.014764 |

Trial 2 figures come from its four native Codex session rollouts and its
`report.json`. The report marks the baseline accepted but the team arm
failed, so these figures do not form an accepted pair. The DeepSeek amount is
an engine estimate, not an invoice.

Trial 1 was interrupted before the native episode usage checkpoint. Its
retained interruption record marks usage **unknown**, not zero; no native
session rollout was available for reconciliation.

The first five saved lifecycle probes failed. Their final persisted engine
spend records provide request-level estimates; they do not prove lifecycle
acceptance:

| Probe | Failure | Uncached input | Cached input | Output | API calls | API-cost estimate |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Room WebSocket did not become ready | 10,699 | 83,200 | 2,893 | 15 | $0.007189 |
| 2 | Unexpected owner decision; automation stopped | 1,741 | 45,696 | 589 | 8 | $0.001508 |
| 3 | Unexpected owner decision; automation stopped | 9,342 | 150,912 | 5,735 | 23 | $0.010600 |
| 4 | Agent did not return to idle after fixture result | 12,932 | 200,704 | 10,745 | 27 | $0.017991 |
| 5 | Agent did not return to idle after fixture result | 11,198 | 93,824 | 2,748 | 16 | $0.007227 |
| **Failed-probe total** | | **45,912** | **574,336** | **22,710** | **89** | **$0.044515** |

The lifecycle estimates are summed from each attempt's final persisted spend
record, rather than summing earlier snapshots in the same room state. They are
provider estimates, not reconciled invoices.

A sixth lifecycle probe passed its 16 checks, including one-use skill release,
MCP schema release, exact approval, persisted approval across pause/restart,
and unchanged source sentinel. Its proof artifact is `proof.json`; its final
room checkpoint is `state.json`. Usage was 5,234 uncached input, 86,656 cached
input, 2,915 output, zero cache write, and 14 API calls. The engine's API-cost
estimate is $0.005594, not an invoice. This is lifecycle acceptance evidence;
it does not make the five earlier failed attempts pass or extend the one-pair
benchmark into a general savings result.

## Accounting correction proof

A fresh protected Codex session made three bounded episodes with GPT-6.1 Sol:
an initial one-word response, an in-process resume with a prompt beginning
`---`, and another resume after reconstructing the provider from its persisted
session record. All three retained the same native session and response.

The per-episode OrgBots deltas summed to **35,083 uncached input, 59,904 cached
input, 250 output, and zero cache-write tokens**, exactly matching both the
persisted cumulative checkpoint and the final native `total_token_usage` event.
This diagnostic usage is separate from the accepted pair and lifecycle probes.
It verifies the accounting correction; it is not a cost-savings trial.

Legacy records without a baseline preserve the native session and mark the
first resumed episode's usage unknown. Partial or decreasing cumulative counters
also remain unknown; they are not added as if they were fresh usage. Existing
historical room event totals are preserved rather than silently rewritten.

## Final dependency-patched build follow-up

An optional seventh lifecycle run on the final package engine timed out after
restart and approval. At the end, the shared allowance ledger reported
insufficient remaining money to reserve another request under the unchanged
$0.25 diagnostic ceiling. That ledger includes conservative reservations and
is not an invoice. The checkpoint contains nine measured requests: 2,131
uncached input, 52,352 cached input, 748 output and a $0.001855 API estimate.
Unreported work is unknown. This attempt is incomplete and is not counted as
a second passing lifecycle proof.

The accepted lifecycle-6 proof covers unchanged lifecycle/approval code. The
final engine's patched dependencies passed the full regression suite and
completed live API requests during probe 7, but a full final-binary lifecycle
rerun is not claimed.
