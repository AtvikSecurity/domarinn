#!/usr/bin/env python3
"""A provider that resolves its own prompts — the case `cache_salt` exists for.

The request carries a `prompt_id`, not prompt text. This program reads the
matching file itself, so from domarinn's side the request is identical whether
the file says "greet warmly" or "be hostile". Nothing in the cache key changes
when that file is edited, which is exactly why the suite digests it explicitly.

This is not a contrived arrangement — it is what happens whenever the system
under test owns its own prompt registry, which is the normal case for an
application being evaluated end to end.
"""

import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent


def main():
    request = json.load(sys.stdin)
    variables = request.get("vars") or {}
    prompt_id = variables.get("prompt_id", "")

    prompt_path = HERE / "prompts" / f"{prompt_id}.md"
    if not prompt_path.is_file():
        json.dump(
            {
                "output": "",
                "error": {
                    "message": f"no such prompt: {prompt_id}",
                    "retriable": False,
                    "class": "provider_bad_request",
                },
            },
            sys.stdout,
        )
        return 0

    instructions = prompt_path.read_text()
    # Stand-in for "send the prompt to a model". Deterministic so the example
    # runs offline; the shape is what matters.
    if "welcome" in instructions:
        text = "Welcome! Happy to help you today."
    elif "specialist" in instructions:
        text = "I'm sorry about this — I'm bringing in a specialist now."
    else:
        text = "Understood."

    json.dump(
        {"output": text, "usage": {"input_tokens": len(instructions.split()), "output_tokens": 8}},
        sys.stdout,
    )
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
