#!/usr/bin/env python3
"""A minimal domarinn `exec` provider, in ~20 lines of dependency-free Python.

This is what the examples use as their system under test. It exists so the
offline examples run for anyone who installed domarinn by any means, with no
toolchain and no API key — and because a real, readable implementation is a
better introduction to the exec protocol than prose.

The full contract is in ../docs/protocol.md. The parts that matter here:

  * domarinn writes exactly one JSON request to stdin, then closes it.
  * You write exactly one JSON response to stdout and exit.
  * stdout is for the response only. Logs and diagnostics go to stderr.
  * Exit 0 whenever you produced a valid response, even one describing a
    failure. A non-zero exit means *your program* broke, and domarinn treats
    that as an infrastructure error rather than a test result.

This provider just echoes its input back, which is enough to exercise
templating, matrix sweeps, file-backed vars and every deterministic assertion
without involving a model.

To use your own system under test, replace the `respond` body with a call into
it and keep the same envelope.
"""

import json
import sys


def respond(request):
    """Echo the most specific thing the suite gave us.

    In order: the rendered prompt if the suite defined one (that is the thing
    under test when prompts are in play), then the `user_input` var, then the
    whole vars map. The fallbacks are what let a suite with no prompts at all -
    a matrix sweep, a file-backed fixture - still produce an output whose
    content the assertions can talk about.
    """
    prompt = request.get("prompt")
    if prompt is not None:
        return prompt
    variables = request.get("vars")
    if isinstance(variables, dict) and "user_input" in variables:
        return variables["user_input"]
    return variables


def main():
    try:
        request = json.load(sys.stdin)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        # A malformed request is domarinn's bug, not ours - but report it as an
        # infrastructure error (non-zero exit) rather than emitting junk.
        print(f"echo-provider: could not parse request: {exc}", file=sys.stderr)
        return 1

    if not isinstance(request, dict):
        print("echo-provider: request was not a JSON object", file=sys.stderr)
        return 1

    json.dump(
        {
            "output": respond(request),
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
        sys.stdout,
    )
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
