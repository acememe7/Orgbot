"""Pure transformation for previewing the versioned specialist team preset."""

from __future__ import annotations

import copy
import json
from pathlib import Path
from typing import Any


PRESET_PATH = Path(__file__).with_name("specialist-team.json")
_MEMORY_START = "<!-- orgbot-specialist-team-routing:start -->"
_MEMORY_END = "<!-- orgbot-specialist-team-routing:end -->"


def load_preset(path: Path = PRESET_PATH) -> dict[str, Any]:
    """Load and validate a preset without accessing the saved app library."""
    with path.open(encoding="utf-8") as source:
        preset = json.load(source)
    validate_preset(preset)
    return preset


def validate_preset(preset: dict[str, Any]) -> None:
    if preset.get("schema_version") != 1:
        raise ValueError("unsupported specialist team schema_version")
    bots = preset.get("bots")
    if not isinstance(bots, list) or len(bots) != 7:
        raise ValueError("specialist team preset must contain exactly 7 bots")

    required_fields = {
        "id", "name", "handle", "persona", "provider", "model", "apiKey",
        "role", "powers",
    }
    ids: set[str] = set()
    handles: set[str] = set()
    for bot in bots:
        if not isinstance(bot, dict) or not required_fields.issubset(bot):
            raise ValueError("each specialist bot must contain all library fields")
        if not all(isinstance(bot[field], str) and bot[field] for field in (
            "id", "name", "handle", "persona", "provider", "model", "role"
        )):
            raise ValueError("specialist bot identity and provider fields must be nonempty strings")
        if bot["id"] in ids or bot["handle"] in handles:
            raise ValueError("specialist bot ids and handles must be unique")
        ids.add(bot["id"])
        handles.add(bot["handle"])
        if bot["apiKey"] != "":
            raise ValueError("preset API keys must be empty")
        powers = bot["powers"]
        if not isinstance(powers, dict) or set(powers) != {
            "may_create_tasks", "may_inspect", "may_submit", "review_only"
        } or not all(isinstance(value, bool) for value in powers.values()):
            raise ValueError("specialist bot powers must be explicit booleans")

    limits = preset.get("room_limits")
    if limits != {"max_turns": 100, "parallel_turns": 1}:
        raise ValueError("specialist team room limits must be 100 turns and one parallel turn")
    if not isinstance(preset.get("routing_memory"), str) or not preset["routing_memory"].strip():
        raise ValueError("specialist team routing_memory must be a nonempty string")


def _with_routing_memory(existing: Any, routing: str) -> str:
    if not isinstance(existing, str):
        existing = ""
    start = existing.find(_MEMORY_START)
    if start >= 0:
        end = existing.find(_MEMORY_END, start + len(_MEMORY_START))
        if end >= 0:
            end += len(_MEMORY_END)
            existing = existing[:start] + existing[end:]
    existing = existing.rstrip()
    block = f"{_MEMORY_START}\n{routing.strip()}\n{_MEMORY_END}"
    return f"{existing}\n\n{block}" if existing else block


def replace_with_specialist_team(
    library: dict[str, Any], preset: dict[str, Any] | None = None
) -> dict[str, Any]:
    """Return a copied library with preset bots seated in every existing room.

    Unknown library fields and room history remain untouched. This function
    does not persist anything and does not read credentials or user data.
    Stable ids in the preset make the same input produce the same output.
    """
    selected = load_preset() if preset is None else copy.deepcopy(preset)
    validate_preset(selected)
    if not isinstance(library, dict):
        raise TypeError("library must be a decoded JSON object")
    rooms = library.get("rooms", [])
    if not isinstance(rooms, list) or any(not isinstance(room, dict) for room in rooms):
        raise ValueError("library rooms must be a list of objects")

    result = copy.deepcopy(library)
    bots = copy.deepcopy(selected["bots"])
    bot_ids = [bot["id"] for bot in bots]
    result["bots"] = bots
    limits = selected["room_limits"]
    routing = selected["routing_memory"]
    for room in result.get("rooms", []):
        room["bots"] = [{"bot_id": bot_id} for bot_id in bot_ids]
        room["max_turns"] = limits["max_turns"]
        room["parallel_turns"] = limits["parallel_turns"]
        room["memory"] = _with_routing_memory(room.get("memory", ""), routing)
    return result
