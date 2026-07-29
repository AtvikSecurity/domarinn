#!/usr/bin/env python3
"""A stand-in for YOUR system under test.

Real ones call a model; this one is deterministic so the example runs offline.
What matters is the shape, which is identical either way:

  * read one JSON request on stdin
  * do whatever your system does
  * write one JSON response on stdout, then exit 0

The request carries `prompt` (rendered, when the suite defines prompts), `vars`,
`params`, `test` and `tools`. The response needs only `output`; `usage` and
`cost_usd` are what make the `tokens:` and `cost:` assertions meaningful, so
report them if you can.

Note the model arrives as an argv flag rather than being read from the
environment. That is deliberate: argv is part of domarinn's cache fingerprint,
so two models can never share a cache entry.
"""

import argparse
import json
import os
import sys


def answer(model, style, question):
    """Whatever your system actually does. Deterministic here."""
    lowered = question.lower()
    if "third time" in lowered:
        body = "I'm sorry for the runaround — I'm escalating this to a specialist now."
    elif "return" in lowered or "refund" in lowered:
        body = "Returns are accepted within 30 days of delivery."
    else:
        body = "Happy to help — could you tell me a little more?"
    if model == "careful" and style == "thorough":
        body += " I'll follow up in writing once it's confirmed."
    return body


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    args = parser.parse_args()

    request = json.load(sys.stdin)
    variables = request.get("vars") or {}
    question = variables.get("user_input", "")
    style = os.environ.get("ASSISTANT_STYLE", "concise")

    text = answer(args.model, style, question)
    json.dump(
        {
            "output": text,
            # Reporting usage is what lets `tokens:` and `cost:` assertions do
            # anything. Omit it and a `cost:` budget passes as "not reported".
            "usage": {"input_tokens": len(question.split()), "output_tokens": len(text.split())},
            # Response metadata, never part of the cache key.
            "model": args.model,
            "stop_reason": "end_turn",
        },
        sys.stdout,
    )
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
