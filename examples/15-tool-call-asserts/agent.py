#!/usr/bin/env python3
"""An exec provider that reports TOOL CALLS as well as text.

The response field is `tool_calls`: a list of {"name": ..., "arguments": {...}},
in the order the model made them. Two rules matter:

  * `arguments` is the DECODED object, not the raw JSON string some vendors put
    on the wire. Forwarding the string hands every assertion a parsing problem
    instead of an argument.
  * When the model called a tool and produced no prose, say so with
    `empty_reason: "tool_use_only"`. Returning "" instead scores every text
    assertion zero for a reason that has nothing to do with the prompt.

This one is rule-based so the example runs offline; a real provider forwards the
`tools` from the request to its model and passes the model's choice back.
"""

import json
import sys


def decide(question, tools):
    """Return (text, tool_calls)."""
    lowered = question.lower()
    offered = {tool["name"] for tool in tools}

    if "refund" in lowered and "issue_refund" in offered:
        return "", [{"name": "issue_refund",
                     "arguments": {"order_id": 77, "amount_usd": 19.99}}]

    if "order" in lowered and "lookup_order" in offered:
        return "", [{"name": "lookup_order",
                     "arguments": {"order_id": 1042, "include_history": False}}]

    return "We're open 9 to 5, Monday through Friday.", []


def main():
    request = json.load(sys.stdin)
    variables = request.get("vars") or {}
    text, calls = decide(variables.get("user_input", ""), request.get("tools") or [])

    response = {
        "output": text,
        "usage": {"input_tokens": 12, "output_tokens": 8},
        "tool_calls": calls,
    }
    if calls and not text:
        # Not an error and not an empty answer — a decision.
        response["empty_reason"] = "tool_use_only"
        response["stop_reason"] = "tool_use"

    json.dump(response, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
