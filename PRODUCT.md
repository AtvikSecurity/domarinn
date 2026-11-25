# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

People running or reviewing LLM evaluations, locally or in CI. The results UI supports both finding a recent or failed run first and starting from a project and suite to inspect its history and baseline.

## Product Purpose

Domarinn runs declarative evaluations and makes the recorded results inspectable. The web UI is the results workspace: find runs, investigate a case or regression, compare results, and inspect the shared cache when needed.

## Capabilities and Constraints

- A run is one recorded evaluation. A set groups runs by project and suite; runs can also lack a set.
- Overview reflects latest CI health. Runs includes CI and local activity; a local run must not silently replace the CI health signal.
- Server cache lookups and a run's cached cases are different metrics. Fully cached runs can still have real verdicts.
- Browser routes are shareable. Existing access rules apply to results, set management, keys, and cache administration.
