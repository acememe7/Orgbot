# Astra, Sol and DeepSeek pilot

This is a paired provider-substitution test, not a single-agent versus team test.
Both variants complete the same two Python modules and use Astra for independent
review and final submission. Sol implements both modules in the baseline. In the
team variant Sol implements the usage normalizer and DeepSeek Flash implements
the persistent task store. Six original unittest cases must pass unchanged.

The variants use the same room limits, fixture, review instructions and serial
scheduler. Sol uses the application's extra-high reasoning setting. Each room
has a fresh native session per Codex agent; subsequent turns resume that session.
Provider-side caches are not cleared. Astra and Sol share one Codex subscription.
One paired trial is a pilot; it cannot establish general savings or subscription
quota percentages. Native CLI provider-call counts represent episodes, which can
contain multiple internal model requests.

Use `baseline.toml` and `team.toml` with the parent benchmark adapter and
`tasks.json` as the fixture. All calls require the runner's explicit paid mode.
Resolve the saved DeepSeek key into the launching process environment; never put
credentials in the templates, report, or command arguments. The local pilot uses
a separate allowance config and ledger so existing room limits are not changed.

Keep raw logs, checkpoints and private CLI profiles outside the repository in a
private directory. Profiles may contain credentials. Commit only sanitized
results. Account for failed attempts separately from completed comparisons and
keep input, cached input, output and cache writes disjoint. Report DeepSeek's
estimated API charge separately from flat-rate Codex token consumption.
