"""Read the shipped executable's embedded frontend without starting its UI/DB."""
from __future__ import annotations
import argparse
import json
import subprocess
from pathlib import Path


def main(arguments: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--build-info", type=Path, required=True)
    options = parser.parse_args(arguments)
    try:
        result = subprocess.run([str(options.exe.resolve()), "--verify-bundle"], capture_output=True, timeout=20, check=True)
        embedded = json.loads(result.stdout.decode("utf-8-sig"))
        expected = json.loads(options.build_info.read_text(encoding="utf-8"))
        if embedded != expected:
            raise ValueError("stale executable")
    except (OSError, ValueError, UnicodeError, subprocess.SubprocessError):
        print("desktop-executable-check: installer executable does not contain the current frontend")
        return 1
    print(f"desktop-executable-check: shipped executable embeds {expected['version']} / {expected['commit'][:12]} and the exact frontend asset manifest")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
