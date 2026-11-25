# Workspace navigation and run history

Keep the existing visual identity and make both common workflows quick: find a recent or failed run, or find a suite and inspect its history. No backend, metric, authorization, route, or case drawer redesign.

## Approaches

A compact top bar preserves horizontal space but still compresses navigation and utilities into one strip. The user chose a labeled sidebar with a compact icon-only mode to recover space. Keep desktop search and account actions in the top bar; put API keys, Admin, and Settings in a secondary group in the rail. Reuse the mobile sheet. Keep version and auth mode in Settings, and keep the mock-data indicator in the shell.

Keeping all tables as the main view preserves precision but makes cross-suite recency hard to scan. A dashboard-only approach would hide the path from signal to evidence. Use a recent-first run activity list and a per-suite pass-rate history alongside the existing exact tables instead.

## Behavior

- Desktop: Overview, Runs, Sets, and Cache stay visible and indicate the current section, including deep links. Collapse to a labeled-for-assistive-tech icon rail; remember the choice in local storage when available. Login and setup remain isolated from workspace navigation. Keep mobile menu and search behavior and the existing single-scrollport contract on fill-viewport pages.
- Runs: default to Activity. `?view=table` keeps the existing grouped table, selection, comparison, sort, and columns. Legacy URL sorts still open in Table unless a view is explicit. Both views share filters, pagination and cached visibility. Activity lists runs globally newest first and links directly to the run and its suite when a set exists.
- Suite: replace the tiny sparkline with a linked history of the newest 16 loaded runs, in sequence from oldest to newest, on a fixed 0–100% scale. Distinguish CI and local with shape as well as color; don't turn a run with no scored cases into 0%. Each plotted run is a keyboard-accessible link with its date, origin, and value available on focus and hover. State that the history covers only a recent loaded window and follows the cached filter. Keep the existing run table and its baseline actions.
- Cache and existing run detail visualizations retain their meaning, routes, and permissions. Make the suite destination apparent on run detail without exposing a broken link for unassigned runs.
