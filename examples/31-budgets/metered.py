#!/usr/bin/env python3
"""An exec provider that reports what it cost.

Three fields make the budget assertions real:

  usage.input_tokens / output_tokens   -> `tokens:`
  usage.cache_write_tokens             -> the `count: billable` distinction
  cost_usd                             -> `cost:`

Cache WRITE tokens are the interesting ones. They are billed at a premium over
an ordinary input token and they are not part of the prompt the model answered,
so a harness that cannot see them under-reports cost on exactly the calls that
populate a cache. Report them and `count: billable` can budget them.

Report `cost_usd` directly when you know it — an exec provider often does,
because it is the thing that actually paid. Without it (and without a `pricing:`
block on a model domarinn has rates for), a `cost:` assertion passes as "cost not
reported" and enforces nothing at all.
"""

import json
import sys

request = json.load(sys.stdin)
what = (request.get("vars") or {}).get("user_input", "")

if what == "cached":
    # A call that POPULATES the prompt cache: a small prompt, a small answer,
    # and 2000 tokens paid to write the cache entry. `total` sees 540; the same
    # call is 2540 `billable`.
    usage = {
        "input_tokens": 500,
        "output_tokens": 40,
        "cache_write_tokens": 2000,
    }
    cost = 0.0009
else:
    usage = {"input_tokens": 12, "output_tokens": 5}
    cost = 0.00004

json.dump(
    {"output": "ok", "usage": usage, "cost_usd": cost, "stop_reason": "end_turn"},
    sys.stdout,
)
sys.stdout.write("\n")
