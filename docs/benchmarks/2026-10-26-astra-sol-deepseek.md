# Astra, Sol and DeepSeek: live pilot, 26 October 2026

**The code passed six unchanged acceptance tests, but the room did not finish
its review workflow. There is no completed pair and no measured savings result.**

All three requested models ran using the existing Codex login and saved
DeepSeek key. The runs exposed launch, reviewer-capability and session-resume
problems. Further paid retries stopped after those findings.

## What ran

The [experiment](../../scripts/benchmarks/experiments/astra-sol-deepseek/README.md)
uses one small Python project with two modules: a provider-token usage normalizer
and a persistent task store. Each module has three acceptance tests. Models may
not change those tests. A separate harness executes all six against the resulting
project; an accepted run additionally requires independent review, submission
and a completed room.

- Baseline: Sol implements both modules; Astra reviews and submits.
- Split team: Sol implements the normalizer; DeepSeek implements the task store;
  Astra reviews and submits.
- Codex CLI 0.164.0; models `gpt-6.1-sol` and `gpt-6-astra`, both observed at
  extra-high reasoning in the native sessions. DeepSeek uses `deepseek-flash`
  (the live catalog identified DeepSeek-V4.1-Flash).
- Serial room scheduling, fresh rooms, 30-turn and ten-minute room limits;
  five-minute native CLI turn timeout. Existing private CLI rules and supported
  tools remain present. No additional agents may be spawned by the fixture.
- This compares provider substitution within reviewed teams, not one unaided
  agent against a team. Astra and Sol share the same Codex subscription.

## Recorded model usage

These are subtotals from successful model episodes within **different failed
attempts**, not a completed baseline/team comparison. Input below excludes cache
reads; the columns are disjoint. No cache-write tokens were reported.

| Attempt | Model and role | Uncached input | Cached input | Output | Estimated API charge |
| --- | --- | ---: | ---: | ---: | ---: |
| 3, baseline | Sol, implements both modules | 66,951 | 842,624 | 8,614 | Codex subscription |
| 3, baseline | Astra, independent review | 76,239 | 509,568 | 5,356 | Codex subscription |
| 1, split team | DeepSeek, task-store implementation | 18,082 | 115,712 | 5,020 | $0.006080 |
| **Recorded task-attempt subtotal** | | **161,272** | **1,467,904** | **18,990** | **$0.006080 API estimate** |

The engine recorded two completed Codex CLI episodes and 15 DeepSeek API
requests. A native CLI episode contains multiple internal model requests, so
those counts cannot be compared directly. Five other engine-tracked CLI attempts
failed without emitting usage; missing values remain unknown, not zero.

DeepSeek's estimate uses the published off-peak Flash rates at the run time:
$0.15 per million uncached input tokens, $0.003 per million cached input tokens,
and $0.60 per million output tokens. The engine sums per-request estimates
rounded to integer microusd, which explains the small difference from applying
the rates once to the aggregate tokens. This is an API estimate, not an exact
invoice reconciliation. The balance endpoint exposes only cents.
[Official DeepSeek pricing](https://api-docs.deepseek.com/quick_start/pricing/).

### Separate setup and debugging usage

Three direct Sol CLI probes outside the protected launch path each returned
`OK` with no tool use. They isolated the account, fixture path and private profile
from the failing sandbox. They used low reasoning and are excluded from the
task-attempt rows above.

| Diagnostic | Uncached input | Cached input | Output | Seconds |
| --- | ---: | ---: | ---: | ---: |
| Normal profile, repository path | 33,602 | 0 | 5 | 10.65 |
| Normal profile, fixture path | 33,249 | 0 | 5 | 10.57 |
| Private profile, fixture path | 26,832 | 0 | 5 | 9.91 |
| **Diagnostic subtotal** | **93,683** | **0** | **15** | |

All recorded task and diagnostic usage together is **254,955 uncached input,
1,467,904 cached input, 19,005 output and zero reported cache-write tokens**.
That is an observed subtotal, not proof of complete metering. It excludes the
engineering conversation and review subagents used to investigate the app;
their consumption was not available to this benchmark. No Codex subscription
quota percentage or hypothetical API cost was measured.

## Attempt outcomes

1. **Attempt 1:** Codex profile preparation failed. Its plugin runtime exceeded
   the previous 256 MiB per-file cap, although the selected profile fit under
   the 1 GiB aggregate cap. The error incorrectly named the aggregate limit.
   DeepSeek completed the task-store implementation; its three original tests
   passed and the task reached review. The full project and room did not pass.
2. **Attempt 2:** The baseline failed after 234.18 seconds because the protected
   process could not resolve provider hosts. Masking `/run` hid the target of
   Ubuntu's `/etc/resolv.conf` symlink. No model usage was emitted. The runner
   stopped before the team arm.
3. **Attempt 3:** Both modules were implemented, and the independent harness
   passed all six unchanged tests. Astra found no blocking code defects against
   the fixture contracts but withheld approval because its review-only role
   could not execute the required combined tests. Sol's next native session
   resume was rejected by the installed CLI argument parser. The baseline
   stopped after **383.85 seconds**, still incomplete; the team arm did not run.

The older attempt-1 report predates the corrected completion checks. Its
process-success status is not an accepted room. The sanitized
[measurement file](2026-10-26-astra-sol-deepseek.json) reconciles all four attempted
arms from their retained engine checkpoints.

## Fixes verified during the test

- Profile copying now admits files up to 512 MiB while retaining the 1 GiB
  aggregate and entry-count limits, with accurate file-versus-tree errors.
- The protected process receives only the resolver file as a read-only bind
  inside its private `/run`. A live DNS-only probe resolved the provider host;
  the host session bus stayed hidden, and writing the resolver was denied.
- The benchmark preserves private workspaces, logs and checkpoints, checks
  room completion as well as test results, and retains partial usage on failure.
  Counts use the usage events' request totals, not the number of events.
- Verification: release engine build passed; three focused Rust profile tests,
  25 workspace helper tests and 18 benchmark harness tests passed.

These fixes were exercised from a source build. The installed GUI/package was
not updated, and no website, release or GitHub publication was performed.

## Remaining blockers and interpretation

The reviewer command restriction and native resume argument construction are
also present in the source revision before this pilot's fixes:

- [Review-only tool filtering](../../core/crates/orgbot-runtime/src/bot.rs)
  deliberately denies `workspace_run`. Simply removing that restriction would
  allow command changes to integrate into the project. Reviewer testing needs
  a disposable execution path whose changes cannot be integrated, or a supported
  tester handoff. The approval requirement was not relaxed to pass this test.
- [Codex resume argument construction](../../core/crates/orgbot-providers/src/agent_cli.rs)
  places model/reasoning flags after session and prompt positionals. That is a
  likely cause of the observed parser rejection, but a corrected invocation
  has not been verified. Session continuity remains a failed live check.
- The saved Codex profile caused unrelated global protocols and broad tool
  context to be loaded for a very small Python task. This is observed overhead;
  the pilot does not isolate its exact token contribution. Cached input is
  repeatedly processed context, not newly generated code.

The next comparable run needs those workflow blockers resolved first. Then
repeat accepted pairs with alternating order and larger representative tasks.
This pilot cannot show that splitting work lowers total tokens, Codex quota,
cash spending, or completion time. It does show that DeepSeek produced the
task-store artifact for a sub-cent estimated API charge, and that current
orchestration can stop despite passing code.

## Evidence handling

Raw logs, checkpoints and private CLI profiles are retained in an owner-private
directory outside Git; profiles may contain credentials. Only sanitized
measurements and fixtures are included here. No key is stored in the templates
or this report.

Source base: `e523c7f6b4a89008bd1c87c91ab4bebfd1e6876f`, plus the profile,
resolver and harness changes described above. Retry engine SHA-256:
`75e050da57d29d8f48a7620cba1eeaa918a41a0ba1f2048baae67eeedd67dd23`.
