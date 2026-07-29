#!/usr/bin/env python3
"""An exec provider with a deliberate delay, so concurrency is observable.

Forty milliseconds is enough that four cases run visibly faster in parallel than
in series, and small enough that this example costs nothing to run in CI.
"""

import json
import sys
import time

request = json.load(sys.stdin)
time.sleep(0.04)
variables = request.get("vars") or {}
json.dump(
    {
        "output": f"handled {variables.get('user_input', '')}",
        "usage": {"input_tokens": 4, "output_tokens": 4},
    },
    sys.stdout,
)
sys.stdout.write("\n")
