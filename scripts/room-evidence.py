#!/usr/bin/env python3
"""Summarise one day of an OrgBots room from its own event log, for the
public daily evidence pages.

    python3 scripts/room-evidence.py HISTORY_DIR 2026-10-30 [--tz-offset 1]

HISTORY_DIR is the room's `.history/<room>` folder (its *.jsonl journals).
The output is JSON with counts and task titles only: no chat text, file
paths, credentials or identifiers beyond task titles, bot handles and the
models each bot ran. Read-only.
"""

import argparse
import collections
import datetime
import glob
import json
import os
import sys

STARTED_WORK = {"claimed", "in_progress"}


def load(history: str) -> list[dict]:
    """Every event in the room's journals, once each, in order."""
    seen: dict[int, dict] = {}
    for path in sorted(glob.glob(os.path.join(history, "*.jsonl"))):
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                try:
                    row = json.loads(line)
                except json.JSONDecodeError:
                    continue
                envelope = row.get("envelope") or {}
                if row.get("type") == "event" and isinstance(envelope.get("seq"), int):
                    seen[envelope["seq"]] = envelope
    return [seen[seq] for seq in sorted(seen)]


def summarise(events: list[dict], day: str, offset_hours: float) -> dict:
    zone = datetime.timezone(datetime.timedelta(hours=offset_hours))
    on_day = lambda e: datetime.datetime.fromtimestamp(e["at"] / 1000, zone).strftime("%Y-%m-%d") == day
    bots, titles = {}, {}
    for event in events:
        body = event["event"]
        if body["event"] == "agent_joined":
            spec = body["spec"]
            provider = spec.get("provider") or {}
            bots[spec["id"]] = {
                "handle": spec["handle"],
                "runs": provider.get("cli") or provider.get("provider"),
                "model": provider.get("model"),
            }
        elif body["event"] == "task_created":
            titles[body["task"]["id"]] = body["task"]["title"]
    today = [e for e in events if on_day(e)]
    turns = collections.Counter()
    output = collections.Counter()
    for event in today:
        body = event["event"]
        if body["event"] == "usage_recorded":
            handle = bots.get(body.get("agent"), {}).get("handle", "unknown")
            turns[handle] += 1
            output[handle] += body.get("usage", {}).get("output_tokens") or 0
    delivered = [
        titles.get(e["event"]["task"], e["event"].get("title", ""))
        for e in today
        if e["event"]["event"] == "task_transitioned" and e["event"]["to"] == "submitted"
    ]
    reviews = [e["event"]["review"] for e in today if e["event"]["event"] == "review_recorded"]
    severities = collections.Counter(f.get("severity") for r in reviews for f in r.get("findings", []))
    sent_back = sum(
        1 for e in today
        if e["event"]["event"] == "task_transitioned"
        and e["event"]["from"] == "in_review" and e["event"]["to"] in STARTED_WORK
    )
    approved = sum(
        1 for e in today
        if e["event"]["event"] == "task_transitioned" and e["event"]["to"] == "approved"
    )
    per_task_rounds = collections.Counter(r.get("task") for r in reviews)
    return {
        "day": day,
        "utc_offset_hours": offset_hours,
        "bots": sorted({(b["handle"], b["runs"], b["model"]) for b in bots.values()}),
        "turns": sum(turns.values()),
        "turns_by_bot": dict(turns.most_common()),
        "output_tokens": sum(output.values()),
        "tasks_created": sum(1 for e in today if e["event"]["event"] == "task_created"),
        "tasks_delivered": len(delivered),
        "delivered": delivered,
        "reviews": len(reviews),
        "approved": approved,
        "sent_back": sent_back,
        "findings_by_severity": dict(severities),
        "most_review_rounds": [
            {"task": titles.get(task, ""), "reviews": count}
            for task, count in per_task_rounds.most_common(3)
        ],
        "owner_questions": sum(1 for e in today if e["event"]["event"] == "decision_asked"),
        "owner_messages": sum(
            1 for e in today
            if e["event"]["event"] == "message_posted"
            and e["event"]["message"].get("author", {}).get("kind") == "human"
        ),
        "task_amendments": sum(1 for e in today if e["event"]["event"] == "task_amended"),
        "errors": sum(1 for e in today if e["event"]["event"] == "agent_errored"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("history")
    parser.add_argument("day", help="YYYY-MM-DD in the chosen time zone")
    parser.add_argument("--tz-offset", type=float, default=1.0, help="hours from UTC; WAT is 1")
    args = parser.parse_args()
    events = load(args.history)
    if not events:
        print("no events found", file=sys.stderr)
        return 1
    json.dump(summarise(events, args.day, args.tz_offset), sys.stdout, indent=2)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
