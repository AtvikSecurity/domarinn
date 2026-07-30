#!/usr/bin/env python3
"""A domarinn test generator: computes cases instead of listing them.

Reads one JSON request on stdin and writes {"tests": [...]} on stdout. Each
entry is an ordinary test — the same shape you would have written inline.

The point of a generator is that coverage cannot drift. Add a locale or a banned
phrase to the suite's `config` and the matching cases appear on the next run,
with no dataset to remember to update.
"""

import json
import sys


def main():
    request = json.load(sys.stdin)
    config = request.get("config") or {}
    banned = config.get("banned_phrases", [])
    locales = config.get("locales", [])

    tests = []

    # One case per banned phrase. `not-icontains` is generated, not written out
    # by hand, so the list stays the single source of truth.
    for phrase in banned:
        slug = phrase.lower().replace(" ", "-")
        tests.append(
            {
                "id": f"banned/{slug}",
                "tags": ["policy"],
                "vars": {"user_input": "Happy to help — here are your options."},
                "assert": [{"type": "not-icontains", "value": phrase}],
            }
        )

    # One case per locale.
    for locale in locales:
        tests.append(
            {
                "id": f"locale/{locale}",
                "tags": ["i18n"],
                "vars": {"user_input": f"locale={locale}"},
                "assert": [{"type": "contains", "value": f"locale={locale}"}],
            }
        )

    json.dump({"tests": tests}, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
