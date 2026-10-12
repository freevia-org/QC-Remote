#!/usr/bin/env python3
"""Select the exact qc_remote_lib test executable from Cargo JSON messages."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any


def select_executable(messages: list[dict[str, Any]]) -> str:
    candidates = [
        message
        for message in messages
        if message.get("reason") == "compiler-artifact"
        and message.get("target", {}).get("name") == "qc_remote_lib"
        and "rlib" in message.get("target", {}).get("kind", [])
        and "rlib" in message.get("target", {}).get("crate_types", [])
        and message.get("profile", {}).get("test") is True
        and isinstance(message.get("executable"), str)
        and bool(message["executable"])
    ]
    if len(candidates) != 1:
        raise ValueError(
            "expected exactly one qc_remote_lib rlib test executable; "
            f"found {len(candidates)}"
        )
    executable = candidates[0]["executable"]
    if "\n" in executable or "\r" in executable:
        raise ValueError("Cargo's selected executable path contains a line break")
    path = Path(executable)
    if not path.is_absolute() or not path.is_file():
        raise ValueError("Cargo's selected test executable is not an existing absolute file")
    return str(path)


def main() -> int:
    try:
        messages = [json.loads(line) for line in sys.stdin if line.strip()]
        executable = select_executable(messages)
        env_file = os.environ.get("GITHUB_ENV")
        if not env_file:
            raise ValueError("GITHUB_ENV is required")
        with open(env_file, "a", encoding="utf-8", newline="\n") as output:
            output.write(f"QC_REMOTE_LIB_TEST_EXECUTABLE={executable}\n")
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"test executable selection failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
