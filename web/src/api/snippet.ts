// Search-snippet match markers — the wire-contract counterpart to
// `SNIPPET_OPEN`/`SNIPPET_CLOSE` in `crates/domarinn-server/src/dto/search.rs`.
// The server's FTS5 `snippet()` wraps each matched token between these two
// private-use-area characters; `<Snippet>` splits on them to render <mark>
// highlights. PUA characters are used because any printable delimiter can
// legitimately occur in stored prompt/output text.

export const SNIPPET_OPEN = "\ue000";
export const SNIPPET_CLOSE = "\ue001";
