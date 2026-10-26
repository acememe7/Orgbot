# Context and input-token audit, 26 October 2026

This audit explains the large Codex input totals in [the retained failed pilot](2026-10-26-astra-sol-deepseek.md). It uses native session usage records and persisted event metadata; it does not reproduce prompts or private session contents.

Codex session logs contain a usage record for each internal model response. Their `input_tokens` includes cached input, so the table below reports **disjoint input = input_tokens − cached_input_tokens**, alongside cache-read and output counts. These are provider-reported token counts, not estimates. The benchmark engine counted one CLI episode per agent; the native logs show 11 Astra and 14 Sol response requests inside those episodes.

| Model | Response requests | First raw input | Last raw input | Last cache-read | Last disjoint input | Total disjoint input | Total cache-read | Total output |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Astra (`gpt-6-astra`) | 11 | 21,957 | 79,530 | 79,232 | 298 | 76,239 | 509,568 | 5,356 |
| Sol (`gpt-6.1-sol`) | 14 | 27,692 | 85,263 | 84,992 | 271 | 66,951 | 842,624 | 8,614 |

The full per-response sequence is in the [machine-readable audit](2026-10-26-context-audit.json). Cache-read grew over successive responses as the conversation and tool history accumulated. The later per-response disjoint input fell to a few hundred tokens, while earlier responses added thousands. Cumulative disjoint input is not a count of unique code: it sums the uncached portion of every request, and may include repeated context.

## Measured prompt and tool surfaces

Native event records provide byte and character counts for logged surfaces, but not a token attribution by source. The following figures therefore describe text sizes only:

- The serialized schema for three assistant tools was 39,259 bytes in each native session.
- The launch metadata recorded a 7,546-byte Astra system brief and a 6,994-byte Sol system brief. Base-instruction strings measured 21,221 and 21,572 bytes respectively.
- Logged developer-message text totaled 51,291 bytes for Astra and 70,028 for Sol. Logged user-message text totaled 10,979 and 8,798 bytes.
- Serialized tool-call arguments totaled 9,561 bytes for Astra and 17,166 for Sol. Persisted tool-result strings totaled 238,575 and 225,704 bytes respectively. Individual captured results reached about 40 KB.
- Tool calls referenced global engineering, master-taste, production-guardrails, AGENTS, design-language, and frontend-craft rules. Captured outputs for those calls ranged from 20,002 to 40,175 bytes for Astra and 17,791 to 40,173 bytes for Sol. The captures include truncation, so these are logged-output sizes, not full rule-file sizes or proof of exact prompt inclusion.

Current sizes of the referenced global files range from 6,209 bytes (`frontend-craft.md`) to 46,132 bytes (`production-guardrails.md`). Design/frontend manuals were unrelated to this Python fixture. Core engineering rules still apply, and production safeguards can apply to its persistence and data preservation. The exact relevance of every section in the broad production/legal document was not measured. Unrelated UI reads, large tool schemas and repeated results are observed sources of context overhead; no per-source token contribution was measured, and byte categories may overlap with other logged surfaces.

These measurements alone do not establish that a rule can safely be removed.
The policy and its bounded follow-up check are described below.

## Task-scoped policy and live follow-up

The shared [system brief](../../core/crates/orgbot-runtime/src/prompt.rs) now tells
agents to retain core engineering, quality, permissions, credentials, file
retention and independent review requirements. Unknown mandatory rules remain
mandatory. Known domain manuals are selected from the actual work:

| Work | Rules to load when available |
| --- | --- |
| UI, interaction or game presentation | Design language and frontend craft |
| Website | Website rules, plus applicable UI rules |
| Unity or Unreal | The matching engine protocol |
| Specified peer networking or matchmaking | The corresponding DEADZONE protocol |
| Security-sensitive work, private data, persistence/databases, billing, deployment or recovery | Production guardrails |

An explicit owner request to read a manual still applies. Agents should use the
existing skill index and load a matching skill body on demand, reuse unchanged
rules within the session, and record their selection in a verification artifact.
The room's assigned reviewers handle review; no duplicate external review team
is required. Rules remain available if the task's scope changes.

This is **instruction-based selection, not an enforced file filter**. It does
not remove CLI-injected base instructions or tool catalogs. No host rules,
profiles or tools are removed. The policy remains in the stable system brief;
changing task details stay in the situation message so they do not unnecessarily
invalidate that prefix.

One fresh Sol episode used the same two-module fixture, baseline roster and
extra-high reasoning. Only the room turn limit changed from 30 to 1. A separate
allowance capped native CLI episodes at one. The room intentionally paused with
both tasks in review, so the harness correctly did not mark the whole room
accepted. No Astra turn, resume attempt or DeepSeek call followed.

**All six original tests passed unchanged.** The native prompt contained the new
policy. Recorded tool calls read the core manuals and production guardrails;
no reads of the UI, website, Unity, Unreal or DEADZONE manuals were observed.
The verification artifact explained the production selection as local
persistence and data preservation. Hash checks confirmed that the host config
and twelve instruction files were unchanged.

| Sol implementation episode | Uncached input | Cached input | Output | Internal responses | Last request's raw input |
| --- | ---: | ---: | ---: | ---: | ---: |
| Original pilot | 66,951 | 842,624 | 8,614 | 14 | 85,263 |
| Task-scoped follow-up | 65,485 | 668,032 | 8,573 | 13 | 71,628 |

The follow-up took 212.42 seconds including startup. It consumed the tokens
shown in its row through the Codex subscription; it made no additional DeepSeek
API calls. These are observed differences between two episodes, not a controlled
estimate of savings caused by the policy. Cache state and implementation choices
differed, and one episode used fewer model requests. This does not measure a
subscription quota percentage or prove savings from splitting providers.

The remaining input is substantial: the CLI still injects its instructions and
skill/tool catalog, and the selected production manual was read more than once.
One call batched several long files and its captured output was truncated;
another requested the production manual again. Loading only relevant sections
of large domain manuals and avoiding repeated oversized results remain areas
to investigate. Exact per-source token attribution is unavailable.

Validation: release engine build, **109 runtime tests** and **30 native-CLI
provider tests** passed. The runtime test suite emitted two existing unused
variable warnings in `restart.rs`. Actual model adherence was checked only with
Codex/Sol. The reviewer-command and resumed-CLI parser blockers from the original
pilot remain unresolved. The installed application/package and public site
were not updated by this source change.

## Separate diagnostics and API arithmetic

Three low-reasoning Sol diagnostics reported 33,602, 33,249, and 26,832 input tokens, with zero cache-read and five output tokens each. They are outside the task attempt and excluded from the per-model table.

The DeepSeek estimate in the pilot report is $0.006080; multiplying dollars by 100 gives **0.608 cents**. This is arithmetic on the reported estimate, not invoice reconciliation.

The pilot did not complete an accepted baseline/team pair. These context measurements explain observed usage; they do not show that splitting work reduces tokens, cost, or time. The direct and fixture-path diagnostic native session files were not present in the retained evidence; their exact usage is preserved in diagnostic stdout. No hidden reasoning, raw prompts, credentials, or private filesystem paths are included here.
