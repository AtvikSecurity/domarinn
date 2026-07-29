#!/usr/bin/env python3
"""A domarinn `exec` assertion: does the invoice total match its line items?

Reads one JSON request on stdin:

    {"domarinn": {...}, "output": <the provider's answer>,
     "vars": {...}, "config": {...}, "test": {...}, "provider": {...}}

and writes one verdict on stdout:

    {"pass": true, "score": 1.0, "reason": "..."}

`reason` is what shows up beside the case in the report, so write it for the
person reading a red build, not for a log.

Exit 0 whenever you produced a verdict — including a failing one. A non-zero
exit means the checker itself broke, which domarinn reports as an
infrastructure error rather than a test failure.
"""

import json
import sys


def verdict(passed, reason, score=None):
    return {
        "pass": passed,
        "score": (1.0 if passed else 0.0) if score is None else score,
        "reason": reason,
    }


def main():
    request = json.load(sys.stdin)
    tolerance = (request.get("config") or {}).get("tolerance", 0.005)

    output = request.get("output")
    if not isinstance(output, str):
        output = json.dumps(output)

    try:
        invoice = json.loads(output)
    except json.JSONDecodeError as exc:
        json.dump(verdict(False, f"output is not JSON: {exc}"), sys.stdout)
        return 0

    try:
        expected = sum(invoice["lines"])
        actual = invoice["total"]
    except (KeyError, TypeError) as exc:
        json.dump(verdict(False, f"missing lines/total: {exc}"), sys.stdout)
        return 0

    drift = abs(expected - actual)
    json.dump(
        verdict(
            drift <= tolerance,
            f"line items sum to {expected:.4f}, total says {actual:.4f} "
            f"(drift {drift:.4f}, tolerance {tolerance})",
        ),
        sys.stdout,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
