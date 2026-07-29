#!/usr/bin/env python3
"""An assertion that must never execute.

It sits behind a deterministic assertion that fails, so domarinn decides the
case before reaching it and records this one as `skipped`. Exiting non-zero is
the point: if short-circuiting ever stopped working, this suite would report an
infrastructure error (exit 3) instead of a plain assertion failure (exit 1), and
the change would be impossible to miss.
"""

import sys

print("short-circuiting failed: this assertion should never have run", file=sys.stderr)
sys.exit(1)
