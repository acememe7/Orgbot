#!/usr/bin/env python3
"""Deterministic offline adapter for exercising artifact verification only.

This adapter writes reference-quality fixture solutions. Its output is tagged
as a fixture source, so the benchmark runner will not treat fabricated token
counts, costs, quality, or savings as model evidence.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path


USAGE_NORMALIZER = '''def normalize_usage(payload, dialect):
    usage = payload.get("usage") or {}
    if dialect == "openai":
        prompt = usage.get("prompt_tokens", 0)
        details = usage.get("prompt_tokens_details") or {}
        cached = details.get("cached_tokens", 0)
        if not all(isinstance(value, int) and value >= 0 for value in (prompt, cached)):
            raise ValueError("OpenAI token counts must be nonnegative integers")
        if cached > prompt:
            raise ValueError("cached prompt tokens exceed prompt total")
        return {
            "input_tokens": prompt - cached,
            "output_tokens": usage.get("completion_tokens", 0),
            "cache_read_tokens": cached,
            "cache_write_tokens": 0,
        }
    if dialect == "anthropic":
        return {
            "input_tokens": usage.get("input_tokens", 0),
            "output_tokens": usage.get("output_tokens", 0),
            "cache_read_tokens": usage.get("cache_read_input_tokens", 0),
            "cache_write_tokens": usage.get("cache_creation_input_tokens", 0),
        }
    raise ValueError("unsupported usage dialect")
'''

TASK_STORE = '''import json
from pathlib import Path
from uuid import uuid4


class TaskStore:
    def __init__(self, path):
        self.path = Path(path)
        if self.path.exists():
            value = json.loads(self.path.read_text(encoding="utf-8"))
            if not isinstance(value, list):
                raise ValueError("task store must contain a list")
            self._tasks = value
        else:
            self._tasks = []

    def _save(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temporary = self.path.with_suffix(self.path.suffix + ".tmp")
        temporary.write_text(json.dumps(self._tasks, sort_keys=True), encoding="utf-8")
        temporary.replace(self.path)

    def create(self, title):
        if not isinstance(title, str) or not title.strip():
            raise ValueError("title must not be empty")
        task_id = uuid4().hex
        self._tasks.append({"id": task_id, "title": title})
        self._save()
        return task_id

    def list_tasks(self):
        return [dict(task) for task in self._tasks]

    def update_title(self, task_id, title):
        if not isinstance(title, str) or not title.strip():
            raise ValueError("title must not be empty")
        for task in self._tasks:
            if task["id"] == task_id:
                task["title"] = title
                self._save()
                return True
        return False

    def delete(self, task_id):
        for index, task in enumerate(self._tasks):
            if task["id"] == task_id:
                del self._tasks[index]
                self._save()
                return True
        return False
'''


def main() -> int:
    case = json.load(sys.stdin)
    workspace = os.environ.get("ORGBOT_BENCH_WORKSPACE")
    if not workspace:
        raise SystemExit("workspace fixture was not supplied")
    root = Path(workspace)
    solutions = {
        "usage-dialect-normalizer": {"src/usage_normalizer.py": USAGE_NORMALIZER},
        "persistent-task-store": {
            "src/__init__.py": "",
            "src/task_store.py": TASK_STORE,
        },
    }
    files = solutions.get(case.get("id"))
    if not files:
        raise SystemExit(f"no offline fixture solution for {case.get('id')!r}")
    for name, content in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    json.dump(
        {
            "deliverable": "Reference fixture implementation written; verifier decides acceptance.",
            "model": "offline-reference-fixture",
            "billing_kind": "local",
            "measurement_source": "fixture",
            "usage_reported": False,
            "usage_complete": False,
            "usage": {},
        },
        sys.stdout,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
