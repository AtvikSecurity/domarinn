#!/usr/bin/env python3
"""An exec provider that looks an order up before it answers.

Same protocol as example 15's agent: `tool_calls` is a list of
{"name": ..., "arguments": {...}} in call order, and `arguments` is the DECODED
object rather than the raw JSON string some vendors put on the wire.

The difference that matters here is that this one fills BOTH channels — a tool
call and a sentence. The sentence is all a rubric normally sees, and on its own
it is indistinguishable from the same sentence guessed. The call is the
evidence, and `include_tool_calls: true` on the grader is what puts it in front
of the judge.

Rule-based so the example runs offline; a real provider forwards the `tools`
from the request to its model and passes the model's choice back.
"""

import json
import re
import sys

# The "order system" this provider stands in for. A real one is a network call;
# what the rubric grades is that it was consulted at all.
ORDERS = {4471: "shipped on Tuesday and arrives Thursday"}


def decide(question, tools):
    """Return (text, tool_calls)."""
    offered = {tool["name"] for tool in tools}
    asked_about = re.search(r"\b(\d{3,})\b", question)

    if not asked_about or "lookup_order" not in offered:
        return "I can check that for you — which order number is it?", []

    order_id = int(asked_about.group(1))
    status = ORDERS.get(order_id, "is still being prepared for dispatch")
    return (
        f"Order {order_id} {status}.",
        [{"name": "lookup_order", "arguments": {"order_id": order_id}}],
    )


def main():
    request = json.load(sys.stdin)
    variables = request.get("vars") or {}
    text, calls = decide(variables.get("user_input", ""), request.get("tools") or [])

    json.dump(
        {
            "output": text,
            "usage": {"input_tokens": 14, "output_tokens": 12},
            "tool_calls": calls,
        },
        sys.stdout,
    )
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
