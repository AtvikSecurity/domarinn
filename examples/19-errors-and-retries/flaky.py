#!/usr/bin/env python3
"""An exec provider that demonstrates every failure shape.

The important field is `error.retriable`. Only the provider knows whether a
failure is worth another attempt, so only the provider can say:

  * a 429 or a 503 is transient  -> retriable: true
  * a rejected API key is not    -> retriable: false

Getting this backwards is expensive in both directions: retrying a bad
credential hammers an endpoint that will never say yes, and giving up on a rate
limit throws away a run that would have succeeded a second later.

`class` names the failure using domarinn's vocabulary so a reader can tell one
error from another without parsing prose, and `retry_after_ms` forwards a
`Retry-After` the child received rather than swallowing it.
"""

import json
import sys


def main():
    request = json.load(sys.stdin)
    what = (request.get("vars") or {}).get("user_input", "")

    if what == "rate-limit":
        response = {
            "output": "",
            "error": {
                "message": "429 Too Many Requests from the upstream model",
                "retriable": True,
                "class": "provider_rate_limit",
                "retry_after_ms": 20,
            },
        }
    elif what == "bad-key":
        response = {
            "output": "",
            "error": {
                "message": "401 Unauthorized: the API key was rejected",
                # Retrying this forever would be the wrong answer.
                "retriable": False,
                "class": "provider_auth",
            },
        }
    elif what == "refuse":
        # NOT an error: the call succeeded and the model declined. Saying so is
        # what lets the suite treat it as a skip instead of a zero.
        response = {
            "output": "",
            "empty_reason": "refusal",
            "stop_reason": "refusal",
            "usage": {"input_tokens": 8, "output_tokens": 0},
        }
    else:
        response = {
            "output": "request handled",
            "usage": {"input_tokens": 8, "output_tokens": 3},
        }

    json.dump(response, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
