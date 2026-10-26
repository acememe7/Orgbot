# OrgBots website and installer

The product site lives in `website/`, with no build step or external runtime
dependencies. Each page works as ordinary HTML; small JavaScript enhancements
handle copying installation commands and viewing product examples. Public
assets contain no room data, credentials or live provider requests.

The installer is `scripts/install-orgbots.sh`. Its source of packages remains
the signed GitHub Pages APT archive. The product subdomain is separately hosted
at `https://orgbots.theaecestudio.com`; it must not replace the APT Pages domain.

## Scope and verification

Changes reach the README, installation and archive guides, installer fixtures,
static pages/assets, hosting configuration and static-site checks. Existing
desktop binaries, saved rooms, provider credentials and other hosted sites are
outside this change.

Run `python3 scripts/test-install-orgbots.py` and
`python3 scripts/check-website.py`, then inspect the site in a browser at
375, 768 and 1440 pixels. Check navigation, command copying, keyboard focus,
privacy controls, product examples and the custom 404. Verify the deployed
HTTPS responses, headers and public installer after publication.

## Updating the evidence

`website/results/index.html` is the public evidence page. Its current figures
come from `docs/LINUX_2_0_PLAN.md`, not a live provider comparison. Preserve this
distinction when editing it. The screenshots use synthetic fixture data.

For real comparisons, use `scripts/benchmarks/README.md` and its existing
baseline/team harness. Record the exact source revision, model identifiers,
provider plans, task fixtures, trial count, run dates and machine environment.
Publish accepted outputs and failures alongside elapsed time, human
interventions, input/output/cache counts, API charges or estimates and unknown
values. Keep subscription usage separate from hypothetical API equivalents.

Before publishing reports, remove credentials, private prompts, machine paths
and user identifiers. Commit a sanitized report under `docs/benchmarks/` and
link that exact commit from the results page. Update installation limitations
only when the corresponding checks have passed. A synthetic run, accelerated
clock or process restart is not paid savings or real multi-day evidence.

## Delivery record

Deployment and browser verification are recorded here when performed. Google
Search Console, analytics configuration, independent backup and restore, and
jurisdiction-specific legal assessment must be reported separately from source
checks; none is established by this document.
