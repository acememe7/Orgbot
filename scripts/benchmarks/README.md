# Paired code-work benchmark

`run.py` gives the same isolated project fixture to a baseline adapter and a team
adapter for repeated, alternating trials. Each run is retained under
`<output>/runs/<variant>-trial-<n>-<case>` with its workspace, room config,
checkpoint, and stdout/stderr logs. Reports are rewritten after each run; a
failed run is recorded and stops the benchmark. The included fixtures are concrete
code work: normalizing provider cache-token dialects and implementing a
persistent task store. Their acceptance comes from Python test commands run
against each adapter's resulting files in a fresh temporary workspace. Adapter
self-reported phrases do not decide acceptance for these fixtures.

Run a no-model harness check with the deterministic fixture adapter:

```sh
python3 scripts/benchmarks/run.py \
  --baseline-cmd 'python3 scripts/benchmarks/fixture_adapter.py' \
  --team-cmd 'python3 scripts/benchmarks/fixture_adapter.py' \
  --trials 3 --output /tmp/orgbots-benchmark-check
```

This checks fixture setup and independent acceptance only. Fixture adapter runs
have unknown usage and costs and are explicitly **not** evidence of savings.

## OrgBots engine adapter

`orgbot_adapter.py` is the real-engine adapter for later baseline/team trials.
It writes the supplied project into the fixture workspace, renders an
owner-reviewed TOML template, invokes the built `orgbot run` binary, then reads
token usage and money only from persisted `UsageRecorded` events in the room
checkpoint. It does not accept adapter-claimed token totals. It reads the
serialized event envelope produced by the Rust engine, groups usage by agent
and provider, sums actual spend for API-backed calls, and keeps subscription
usage separate from hypothetical API-equivalent cost.

Copy or review `orgbot-room-template.toml`, then configure both adapters with
the exact same provider configuration and separate baseline/team room policies.
The `--provider-toml` value is an inline TOML `ProviderConfig` table tagged by
`kind`. For a local smoke-test provider, use `{ kind = "scripted", script = "..." }`; for an
installed agent CLI, provide its CLI kind, model, workdir, and policy. The
placeholder `{{WORKSPACE}}` must not be surrounded by quotes because the
adapter inserts a TOML-quoted path. Remote providers (including agent CLIs) are
blocked unless the runner is explicitly started with both `--mode paid
--allow-paid`. No live provider run is part of the repository acceptance
checks.

For a multi-agent comparison, use
`orgbot-team-room-template.toml`. It defines a Codex coordinator, Claude CLI
frontend worker, and DeepSeek API backend. Supply full valid `ProviderConfig`
tables through repeatable `--template-value NAME=VALUE` arguments. The owner
can change agent count, model IDs, role descriptions, and provider settings in
a copied template; there is no adapter-imposed fixed team configuration.

Example shape (substitute the reviewed template and provider settings):

```sh
python3 scripts/benchmarks/run.py \
  --baseline-cmd 'python3 scripts/benchmarks/orgbot_adapter.py --config-template scripts/benchmarks/orgbot-room-template.toml --provider-toml '\''{ kind = "cli_agent", cli = "codex", model = "MODEL_ID", workdir={{WORKSPACE}}, policy = "workspace_write" }'\''' \
  --team-cmd 'python3 scripts/benchmarks/orgbot_adapter.py --config-template scripts/benchmarks/orgbot-room-template.toml --provider-toml '\''{ kind = "cli_agent", cli = "codex", model = "MODEL_ID", workdir={{WORKSPACE}}, policy = "workspace_write" }'\''' \
  --trials 5 --mode paid --allow-paid --output benchmark-report
```

That command is an example for a future owner-authorized paid run. It was not
run as part of local verification.

Use `--rates rates.json` only when the checkpoint lacks a configured
API-equivalent estimate. Rates are four integer microusd-per-million fields per
model: `input_microusd_per_million`, `output_microusd_per_million`,
`cache_read_microusd_per_million`, and `cache_write_microusd_per_million`.
Unknown usage or rates remain unknown. Reports include each trial, latency,
independent acceptance, usage completeness, actual API estimate,
API-equivalent estimate, and human intervention count when persisted in the
engine log. Compare accepted work alongside cost; do not infer savings from
synthetic runs or from unaccepted outputs.

The [Astra/Sol/DeepSeek live pilot](../../docs/benchmarks/2026-10-26-astra-sol-deepseek.md)
records actual usage and workflow failures. It did not produce a completed
comparison and provides no savings claim.
