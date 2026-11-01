#!/usr/bin/env python3
"""Render the public build log at website/build/ from builds.json.

The list is written into the HTML, so it reads without JavaScript and search
engines see it; build.js only adds the filters, the card switching and the 3D
room. Run after adding a build:

    python3 scripts/build-log/render.py

It also writes build.html at the repository root, a copy that opens from disk.
It prints the SHA-256 of the page's JSON-LD; the site's Content-Security-Policy
in deploy/website/nginx-orgbots.*.conf must list it.
"""

from __future__ import annotations

import base64
import hashlib
import html
import json
from collections import Counter, OrderedDict
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DATA = Path(__file__).with_name("builds.json")
OUT = ROOT / "website" / "build" / "index.html"
LOCAL = ROOT / "build.html"
REPO = "https://github.com/acememe7/Orgbot"
SITE = "https://orgbots.theaecestudio.com"
VERSION = "2.2.1"
ASSET_VERSION = "20261101b"

KINDS = OrderedDict(
    [
        ("all", "All"),
        ("release", "Versions"),
        ("r220", "2.2.0 rebuilds"),
        ("r221", "2.2.1 rebuilds"),
        ("site", "Website"),
        ("origin", "Beginnings"),
    ]
)

ROSTER = [
    ("opus", "architect, designer, product manager, coordinator"),
    ("astra", "security, reviewer, coordinator"),
    ("sol", "engineer, ops"),
    ("mira", "engineer, frontend"),
    ("grok-quick", "engineer, generalist"),
    ("deepseek-builder", "engineer, backend, data"),
    ("ollama-glm", "QA"),
    ("ollama-deepseek", "researcher, analyst"),
    ("ellis", "writer, editor, documenter"),
    ("grok", "support"),
]

SCENARIOS = [
    ("run", "A full run"),
    ("handoff", "Handoffs"),
    ("ask", "Asking you"),
    ("land", "Review and land"),
    ("removal", "Removal review"),
]

STEPS = [
    "You post a goal. With no bot named, it reaches one coordinator.",
    "The coordinator splits it into tasks on the shared board.",
    "Bots claim the tasks that match their roles and walk them back to their own copy of your project.",
    "Each bot works and commits inside its copy. Your project is not touched.",
    "A bot offers another an assist. If that bot is busy, the offer waits for it instead of expiring.",
    "A bot that needs a sign-in or a risky command asks you, saying why, what it will run and what happens if you say no. A yes lasts for that task only.",
    "A reviewer checks the work. Changes go back to the author; an approval clears the way.",
    "Approved commits land on your branch. Files a bot deleted wait at a gate until you approve them, folder by folder if you like.",
    "When nothing needs doing, bots rest and their check-ins fold into one line.",
]


def esc(text: str) -> str:
    return html.escape(text, quote=True)


def kind_of(build: dict) -> str:
    if build["kind"] == "rebuild":
        return "r220" if build.get("line") == "2.2.0" else "r221"
    return build["kind"]


def when_of(build: dict) -> datetime | None:
    parts = build["when"].split()
    if len(parts) < 3:
        return None
    return datetime.strptime(f"{parts[0]} {parts[1]} 2026 {parts[2]}", "%d %b %Y %H:%M")


def day_label(day: str, long: bool = False) -> str:
    moment = datetime.strptime(day, "%Y-%m-%d")
    return moment.strftime("%-d %B %Y" if long else "%-d %b")


def commit_link(build: dict) -> str:
    return f'<a class="commit" href="{REPO}/commit/{esc(build["commit"])}">commit {esc(build["commit"])}</a>'


def facts(build: dict) -> str:
    if not build.get("facts"):
        return ""
    items = "".join(f"<li>{esc(fact)}</li>" for fact in build["facts"])
    return f'<ul class="facts" aria-label="Checks for this build">{items}</ul>'


def slide(build: dict, index: int, total: int) -> str:
    kind = kind_of(build)
    why = f'<p class="slide-why"><span>What the room showed</span>{esc(build["why"])}</p>' if build.get("why") else ""
    return f"""
        <article class="slide{' is-active' if index == 0 else ''}" id="slide-{index}" role="tabpanel" aria-roledescription="slide" aria-label="{index + 1} of {total}"{'' if index == 0 else ' hidden'}>
          <p class="slide-meta"><span class="badge k-{kind}">{esc(build["tag"])}</span><time>{esc(build["when"])}</time></p>
          <h3>{esc(build["title"])}</h3>
          <p class="slide-what">{esc(build["what"])}</p>
          {why}
          <div class="slide-foot">{facts(build)}{commit_link(build)}</div>
        </article>"""


def build_item(build: dict) -> str:
    kind = kind_of(build)
    ident = f"b-{build['commit']}"
    search = " ".join(
        [build["tag"], build["title"], build["what"], build.get("why", ""), build["commit"], " ".join(build.get("facts", []))]
    ).lower()
    why = f'<p class="why"><strong>What the room showed:</strong> {esc(build["why"])}</p>' if build.get("why") else ""
    return f"""
          <li class="build k-{kind}" data-kind="{kind}" data-day="{build["day"]}" data-search="{esc(search)}">
            <h4 class="build-head"><button class="build-toggle" type="button" aria-expanded="false" aria-controls="{ident}"><span class="badge k-{kind}">{esc(build["tag"])}</span><span class="build-title">{esc(build["title"])}</span><time class="build-when">{esc(build["when"].split(" ", 2)[2] if len(build["when"].split()) > 2 else build["when"])}</time><svg class="chev" viewBox="0 0 16 16" aria-hidden="true"><path d="M4 6l4 4 4-4" /></svg></button></h4>
            <div class="build-body" id="{ident}"><div class="build-inner">
              <p>{esc(build["what"])}</p>
              {why}
              <div class="build-foot">{facts(build)}{commit_link(build)}</div>
            </div></div>
          </li>"""


def main() -> None:
    builds = json.loads(DATA.read_text(encoding="utf-8"))
    total = len(builds)
    rebuilds = [b for b in builds if b["kind"] == "rebuild"]
    first, last = when_of(rebuilds[0]), when_of(rebuilds[-1])
    rebuild_hours = round((last - first).total_seconds() / 3600)
    latest = builds[-1]
    latest_facts = {fact.split()[0]: fact.split()[1] for fact in latest.get("facts", []) if len(fact.split()) == 3}
    rust_tests = latest_facts.get("Rust", "978")
    app_tests = latest_facts.get("App", "411")

    counts = Counter(kind_of(b) for b in builds)
    counts["all"] = total
    chips = "".join(
        f'<button class="chip{" is-on" if key == "all" else ""}" type="button" data-kind="{key}" aria-pressed="{"true" if key == "all" else "false"}">'
        f'{"" if key == "all" else f"<i class=dot-{key}></i>"}{esc(label)}<span class="chip-count">{counts.get(key, 0)}</span></button>'
        for key, label in KINDS.items()
    )

    days: OrderedDict[str, list[dict]] = OrderedDict()
    for build in builds:
        days.setdefault(build["day"], []).append(build)
    most = max(len(v) for v in days.values())
    bars = []
    for day, items in days.items():
        per = Counter(kind_of(b) for b in items)
        segments = "".join(
            f'<span class="seg seg-{k}" data-n="{per[k]}"></span>' for k in ["origin", "release", "r220", "r221", "site"] if per[k]
        )
        bars.append(
            f'<li><button class="day" type="button" data-day="{day}" data-n="{len(items)}" data-max="{most}" aria-pressed="false" '
            f'aria-label="{day_label(day)}: {len(items)} {"build" if len(items) == 1 else "builds"}">'
            f'<span class="day-bar">{segments}</span><span class="day-n">{len(items)}</span><span class="day-label">{day_label(day)}</span></button></li>'
        )

    groups = []
    for day, items in reversed(days.items()):
        rows = "".join(build_item(b) for b in reversed(items))
        groups.append(
            f"""
        <section class="day-group" data-day="{day}" aria-labelledby="d-{day}">
          <h3 id="d-{day}"><span>{day_label(day, long=True)}</span><span class="day-group-n">{len(items)} {"build" if len(items) == 1 else "builds"}</span></h3>
          <ol class="builds">{rows}
          </ol>
        </section>"""
        )

    recent = list(reversed(builds[-6:]))
    slides = "".join(slide(b, i, len(recent)) for i, b in enumerate(recent))
    tabs = "".join(
        f'<button class="dot{" is-active" if i == 0 else ""}" type="button" role="tab" aria-selected="{"true" if i == 0 else "false"}" aria-controls="slide-{i}" tabindex="{0 if i == 0 else -1}"><span class="dot-tag">{esc(b["tag"])}</span><span class="dot-bar"><span></span></span></button>'
        for i, b in enumerate(recent)
    )

    cast = "".join(
        f'<li><button type="button" data-target="bot:{handle}"><span class="cast-dot"></span><span class="cast-name">@{handle}</span><span class="cast-role">{esc(roles)}</span></button></li>'
        for handle, roles in ROSTER
    )
    cast += (
        '<li><button type="button" data-target="owner"><span class="cast-dot cast-owner"></span><span class="cast-name">You</span><span class="cast-role">the owner</span></button></li>'
        '<li><button type="button" data-target="project"><span class="cast-dot cast-project"></span><span class="cast-name">Your project</span><span class="cast-role">where approved work lands</span></button></li>'
        '<li><button type="button" data-target="board"><span class="cast-dot cast-board"></span><span class="cast-name">The board</span><span class="cast-role">every task and who holds it</span></button></li>'
    )
    scenario_buttons = "".join(
        f'<button type="button" class="scenario{" is-on" if i == 0 else ""}" data-scenario="{key}" aria-pressed="{"true" if i == 0 else "false"}">{esc(label)}</button>'
        for i, (key, label) in enumerate(SCENARIOS)
    )
    steps = "".join(f"<li>{esc(step)}</li>" for step in STEPS)

    ld = json.dumps(
        {
            "@context": "https://schema.org",
            "@type": "CollectionPage",
            "name": "OrgBots build log",
            "url": f"{SITE}/build/",
            "description": "Every OrgBots release and rebuild, with what changed, what the live room showed and the commit.",
            "isPartOf": {"@type": "WebSite", "name": "OrgBots", "url": f"{SITE}/"},
            "about": {
                "@type": "SoftwareApplication",
                "name": "OrgBots",
                "applicationCategory": "DeveloperApplication",
                "operatingSystem": "Ubuntu 24.04",
                "softwareVersion": VERSION,
            },
        },
        separators=(",", ":"),
    )
    ld_hash = base64.b64encode(hashlib.sha256(ld.encode("utf-8")).digest()).decode()

    title = "OrgBots build log: every release and rebuild | OrgBots"
    description = (
        f"All {total} OrgBots builds since the first engine commit: what changed, what the live ten-bot room showed "
        "that made it necessary, and the commit. Plus a 3D walk through how a room runs."
    )
    page = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{esc(title)}</title><meta name="description" content="{esc(description)}"><link rel="canonical" href="{SITE}/build/"><meta property="og:type" content="website"><meta property="og:site_name" content="OrgBots"><meta property="og:title" content="OrgBots build log"><meta property="og:description" content="Every OrgBots release and rebuild, and the live-room problem behind each one."><meta property="og:url" content="{SITE}/build/"><meta property="og:image" content="{SITE}/assets/orgbots-share.jpg"><meta property="og:image:alt" content="OrgBots room usage screen"><meta name="twitter:card" content="summary_large_image"><meta name="twitter:title" content="OrgBots build log"><meta name="twitter:description" content="Every OrgBots release and rebuild, and the live-room problem behind each one."><meta name="twitter:image" content="{SITE}/assets/orgbots-share.jpg"><meta name="theme-color" content="#f2efe7"><link rel="icon" href="../assets/favicon.ico" sizes="any"><link rel="icon" type="image/png" sizes="64x64" href="../assets/favicon.png"><link rel="apple-touch-icon" href="../assets/apple-touch-icon.png"><link rel="stylesheet" href="../styles.css?v=20261101b"><link rel="stylesheet" href="build.css?v={ASSET_VERSION}"><noscript><link rel="stylesheet" href="noscript.css?v={ASSET_VERSION}"></noscript><script defer src="../site.js?v=20261101b"></script><script defer src="build.js?v={ASSET_VERSION}"></script>
  <script type="application/ld+json">{ld}</script>
</head>
<body class="build-page"><a class="skip-link" href="#main">Skip to content</a><header class="site-header"><div class="header-inner"><a class="brand" href="/" aria-label="OrgBots home"><img src="../assets/favicon.png" width="38" height="38" alt=""><span>OrgBots</span></a><button class="menu-toggle" type="button" aria-expanded="false" aria-controls="primary-nav">Menu</button><nav class="nav" id="primary-nav" aria-label="Main navigation"><a href="/results/">Evidence</a><a href="/evidence/">Daily log</a><a href="/build/" aria-current="page">Build log</a><a href="/install/">Installation</a><a href="https://github.com/acememe7/Orgbot">Source code</a><a class="nav-cta" href="/install/">Install for Ubuntu</a></nav></div></header>
<main id="main">
  <section class="bl-hero wrap" aria-labelledby="page-title">
    <div class="bl-hero-copy">
      <p class="eyebrow">Build log</p>
      <h1 id="page-title">Every build, and the room that asked for it</h1>
      <p class="bl-lede">OrgBots is built against a live room of ten AI bots doing real work on a real project. When the room shows a problem, the next build fixes it. Here is each one: what changed, what the room showed, and the commit you can read.</p>
      <dl class="bl-stats">
        <div><dt>builds since the first commit</dt><dd data-count="{total}">{total}</dd></div>
        <div><dt>rebuilds in {rebuild_hours} hours</dt><dd data-count="{len(rebuilds)}">{len(rebuilds)}</dd></div>
        <div><dt>Rust tests passing</dt><dd data-count="{rust_tests}">{rust_tests}</dd></div>
        <div><dt>app tests passing</dt><dd data-count="{app_tests}">{app_tests}</dd></div>
      </dl>
      <p class="bl-proof">Test counts are from the <a href="{REPO}/blob/main/docs/RELEASE_2_2_1_VERIFICATION.md">2.2.1 release record</a>, the latest build's run.</p>
      <div class="actions"><a class="button" href="/install/">Install OrgBots {VERSION}</a><a class="text-link" href="#room">See how a room runs</a></div>
    </div>
    <section class="latest" aria-roledescription="carousel" aria-labelledby="latest-title">
      <div class="latest-head">
        <h2 id="latest-title">Latest builds</h2>
        <div class="latest-controls">
          <button class="icon-btn" type="button" data-latest="prev" aria-label="Previous build"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M10 3L5 8l5 5" /></svg></button>
          <button class="icon-btn latest-play" type="button" data-latest="toggle" aria-label="Pause automatic switching"><svg class="i-pause" viewBox="0 0 16 16" aria-hidden="true"><path d="M5 3v10M11 3v10" /></svg><svg class="i-play" viewBox="0 0 16 16" aria-hidden="true"><path d="M5 3l8 5-8 5z" /></svg></button>
          <button class="icon-btn" type="button" data-latest="next" aria-label="Next build"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6 3l5 5-5 5" /></svg></button>
        </div>
      </div>
      <div class="latest-viewport">{slides}
      </div>
      <div class="latest-tabs" role="tablist" aria-label="Choose a build">{tabs}</div>
    </section>
  </section>

  <section class="room" id="room" aria-labelledby="room-title">
    <div class="wrap">
      <div class="room-head">
        <div>
          <p class="eyebrow">In 3D</p>
          <h2 id="room-title">How a room runs</h2>
        </div>
        <p>Ten bots share one board. Each works in its own copy of your project, hands work to the others, asks you before anything risky, and lands only what a reviewer approved. This is an illustration of that flow with the live room's real roster, not a recording.</p>
      </div>
      <div class="room-scenarios" role="group" aria-label="Choose what to watch">{scenario_buttons}</div>
      <div class="room-grid">
        <div class="room-stage" id="room-stage" data-scene-src="scene.js?v={ASSET_VERSION}">
          <div class="room-skeleton" aria-hidden="true"><span class="sk-board"></span><span class="sk-ring"></span><span class="sk-line sk-a"></span><span class="sk-line sk-b"></span></div>
          <canvas id="room-canvas" aria-hidden="true"></canvas>
          <div class="room-labels" id="room-labels" aria-hidden="true"></div>
          <div class="room-hud">
            <p class="room-caption" id="room-caption">Loading the room.</p>
            <div class="room-transport">
              <button class="icon-btn" type="button" data-room="toggle" aria-label="Pause the room"><svg class="i-pause" viewBox="0 0 16 16" aria-hidden="true"><path d="M5 3v10M11 3v10" /></svg><svg class="i-play" viewBox="0 0 16 16" aria-hidden="true"><path d="M5 3l8 5-8 5z" /></svg></button>
              <button class="icon-btn" type="button" data-room="step" aria-label="Next step"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 3l6 5-6 5zM12 3v10" /></svg></button>
              <button class="icon-btn" type="button" data-room="restart" aria-label="Start this run again"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 8a4.5 4.5 0 1 0 1.6-3.4M3 2.5v3h3" /></svg></button>
              <button class="speed" type="button" data-room="speed" aria-label="Playback speed">1x</button>
            </div>
          </div>
          <div class="room-fallback" id="room-fallback" hidden><p><strong>This browser could not start the 3D view.</strong> The same run is written out below the room, step by step.</p></div>
        </div>
        <aside class="room-side" aria-label="About the room">
          <div class="room-info" id="room-info" aria-live="polite"><h3>Pick anyone</h3><p>Choose a bot, you, your project or the board, in the list below or in the room, to see what it does.</p></div>
          <h3 class="room-side-h">What just happened</h3>
          <ol class="room-log" id="room-log"><li class="log-empty">The run starts when the room loads.</li></ol>
          <h3 class="room-side-h">Who is in the room</h3>
          <ul class="room-cast" id="room-cast">{cast}</ul>
        </aside>
      </div>
      <details class="room-steps"><summary>The same run, written out</summary><ol>{steps}</ol></details>
    </div>
  </section>

  <section class="bl-all wrap" id="builds" aria-labelledby="all-title">
    <div class="section-heading">
      <h2 id="all-title">Every build</h2>
      <p>Filter by kind or day, search for a fix, and open any card for what the room showed.</p>
    </div>
    <div class="days" aria-label="Builds per day"><ol>{''.join(bars)}</ol></div>
    <div class="filters">
      <div class="chips" role="group" aria-label="Filter by kind">{chips}</div>
      <label class="search"><span class="visually-hidden">Search builds</span><svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5L14 14" /></svg><input id="build-search" type="search" placeholder="Search, for example removals" autocomplete="off"></label>
    </div>
    <p class="filter-status" id="filter-status" role="status">Showing all {total} builds.</p>
    <div class="timeline" id="timeline">{''.join(groups)}
    </div>
    <div class="no-match" id="no-match" hidden><p>No build matches that. Try another word, or show everything again.</p><button class="button button-quiet" type="button" data-clear>Show every build</button></div>
  </section>

  <section class="bl-close surface-band" aria-labelledby="close-title">
    <div class="wrap bl-close-inner">
      <div>
        <h2 id="close-title">Run a room of your own</h2>
        <ul class="bl-close-list">
          <li><strong>Ubuntu 24.04, amd64.</strong> Installed from a signed APT archive, the same one these builds were published to.</li>
          <li><strong>Nothing lands without you.</strong> Bots work in private copies; sign-ins, pushes, risky commands and removals wait for your answer.</li>
          <li><strong>Read every line.</strong> The source is on GitHub under the PolyForm Shield License 1.0.0.</li>
        </ul>
      </div>
      <div class="bl-close-cta"><a class="button" href="/install/">Install OrgBots {VERSION}</a><p>Version {VERSION}, rebuilt {esc(latest["when"])}.</p></div>
    </div>
  </section>
<div class="privacy-notice" role="region" aria-label="Privacy choices" data-analytics-enabled="false" hidden><p>Optional analytics is disabled. Hosting security logs may apply. <a href="/privacy/">Privacy policy</a></p><div class="privacy-actions"><button type="button" data-analytics-choice="reject" hidden>Reject analytics</button><button type="button" data-analytics-choice="allow" hidden>Allow analytics</button><button type="button" data-dismiss-notice>Close</button></div><p data-privacy-status role="status"></p></div></main>
<footer class="site-footer"><div class="footer-inner"><p class="footer-note">OrgBots · © 2026 The AECE Studio<br>Linux {VERSION} · Ubuntu 24.04 amd64</p><nav class="footer-links" aria-label="Footer"><a href="/results/">Evidence</a><a href="/evidence/">Daily log</a><a href="/build/" aria-current="page">Build log</a><a href="/install/">Installation</a><a href="/privacy/">Privacy</a><a href="/terms/">Terms</a><a href="mailto:theaece@theaecestudio.com">Contact</a><button class="privacy-reopen" type="button" data-open-notice>Privacy choices</button></nav></div></footer></body></html>
"""
    if "—" in page or "–" in page:
        raise SystemExit("the page holds an em or en dash; the site's copy rules forbid them")
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(page, encoding="utf-8")
    # A copy that opens from disk: its relative links resolve into website/build/.
    LOCAL.write_text(page.replace("<head>", '<head><base href="website/build/">', 1), encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)} and {LOCAL.relative_to(ROOT)}: {total} builds")
    print(f"JSON-LD sha256-{ld_hash}")


if __name__ == "__main__":
    main()
