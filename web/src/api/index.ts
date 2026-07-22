// Barrel over the generated API contract (web/src/api/generated/, ts-rs
// output, CI drift-checked). Every export here is either a straight
// re-export of a generated type or a pure `type X = Y` rename — nothing in
// this file declares a shape of its own, so tsc still enforces the real
// wire contract end to end. Prefer importing from here over reaching into
// `./generated/<Type>` directly; it keeps the 15+ importers of this module
// resilient to generated files being split/renamed.

export type { AuthMode } from "./generated/AuthMode";
export type { MetaCacheLimits } from "./generated/MetaCacheLimits";
export type { MetaResponse } from "./generated/MetaResponse";
export type { SsoProviderMeta } from "./generated/SsoProviderMeta";
export type { SsoKind } from "./generated/SsoKind";

export type { IdentitySource } from "./generated/IdentitySource";
export type { MeResponse } from "./generated/MeResponse";
export type { MeUser } from "./generated/MeUser";
export type { Role } from "./generated/Role";
export type { Scope } from "./generated/Scope";
/** Rename for readability at call sites that talk about API-key/user scope. */
export type { Scope as AuthScope } from "./generated/Scope";

export type { AuthSessionResponse } from "./generated/AuthSessionResponse";
export type { OkResponse } from "./generated/OkResponse";
export type { CredentialsBody } from "./generated/CredentialsBody";

export type { ApiKeyCreatedResponse } from "./generated/ApiKeyCreatedResponse";
export type { ApiKeyId } from "./generated/ApiKeyId";
export type { ApiKeyListResponse } from "./generated/ApiKeyListResponse";
export type { ApiKeyView } from "./generated/ApiKeyView";
export type { CreateKeyBody } from "./generated/CreateKeyBody";

export type { CreateUserBody } from "./generated/CreateUserBody";
export type { PatchUserBody } from "./generated/PatchUserBody";
export type { UserId } from "./generated/UserId";
export type { UserListResponse } from "./generated/UserListResponse";
export type { UserView } from "./generated/UserView";
export type { UserIdentityView } from "./generated/UserIdentityView";

export type { AssertName } from "./generated/AssertName";
export type { AssertResult } from "./generated/AssertResult";
export type { AssertStatus } from "./generated/AssertStatus";
export type { CaseAssertLean } from "./generated/CaseAssertLean";
export type { CaseKey } from "./generated/CaseKey";
export type { CaseListItem } from "./generated/CaseListItem";
export type { CaseListResponse } from "./generated/CaseListResponse";
export type { CaseResult } from "./generated/CaseResult";
export type { CaseStatus } from "./generated/CaseStatus";
export type { CellKey } from "./generated/CellKey";
export type { Output } from "./generated/Output";
export type { TokenUsage } from "./generated/TokenUsage";
// Schema-v2 rendered-prompt shapes (the drawer's Prompt section). `RenderedPrompt`
// is `{ text } | { messages }`; each `ChatMessage` carries a `ChatRole`.
export type { RenderedPrompt } from "./generated/RenderedPrompt";
export type { ChatMessage } from "./generated/ChatMessage";
export type { ChatRole } from "./generated/ChatRole";

export type { IngestResponse } from "./generated/IngestResponse";
export type { RunDetailResponse } from "./generated/RunDetailResponse";
export type { RunId } from "./generated/RunId";
export type { RunListItem } from "./generated/RunListItem";
export type { RunListResponse } from "./generated/RunListResponse";
export type { SearchResponse } from "./generated/SearchResponse";
export type { RunSearchHit } from "./generated/RunSearchHit";
export type { CaseSearchHit } from "./generated/CaseSearchHit";
export type { RunStatusFilter } from "./generated/RunStatusFilter";

// Provider × prompt × test matrix (`GET /runs/{id}/matrix`). Columns are the
// complete, first-seen `(provider, prompt)` set; rows are tests and paginate.
export type { MatrixResponse } from "./generated/MatrixResponse";
export type { MatrixColumn } from "./generated/MatrixColumn";
export type { MatrixRow } from "./generated/MatrixRow";
export type { MatrixCell } from "./generated/MatrixCell";

export type { CompareCaseRow } from "./generated/CompareCaseRow";
export type { CompareDelta } from "./generated/CompareDelta";
export type { CompareResponse } from "./generated/CompareResponse";
export type { CompareSummary } from "./generated/CompareSummary";
export type { BaselineBody } from "./generated/BaselineBody";

// Compare enrichment (Task 4 wire shapes): McNemar significance + Wilson pass
// rates (`CompareStats`/`WilsonView`), server-authoritative per-run aggregate
// totals (`CompareTotals`/`RunTotals`), config-digest drift (`CompareConfig`),
// per-row assert flips (`AssertFlip`), and the `GET /runs/{id}/config` payload.
export type { CompareStats } from "./generated/CompareStats";
export type { WilsonView } from "./generated/WilsonView";
export type { CompareTotals } from "./generated/CompareTotals";
export type { RunTotals } from "./generated/RunTotals";
export type { CompareConfig } from "./generated/CompareConfig";
export type { AssertFlip } from "./generated/AssertFlip";
export type { RunConfigResponse } from "./generated/RunConfigResponse";

// Run-diff view (per-case deltas + aggregate summary + McNemar significance).
// Generated by ts-rs but not previously surfaced here; the matrix/config-drift
// tasks import these from the barrel.
export type { RunDiff } from "./generated/RunDiff";
export type { DiffSummary } from "./generated/DiffSummary";
export type { McNemarView } from "./generated/McNemarView";
export type { CaseDelta } from "./generated/CaseDelta";
export type { Delta } from "./generated/Delta";

export type { ProjectListItem } from "./generated/ProjectListItem";
export type { ProjectsResponse } from "./generated/ProjectsResponse";
export type { SuitePoint } from "./generated/SuitePoint";
export type { SuiteSummary } from "./generated/SuiteSummary";
export type { SuitesResponse } from "./generated/SuitesResponse";

// Per-case history timeline (`GET
// /projects/{project}/suites/{suite}/cases/{case_key}/history`). `points` are
// newest-first; each point's `output_changed` is vs the next-older point
// (`points[i + 1]`), null for the oldest returned point.
export type { CaseHistoryResponse } from "./generated/CaseHistoryResponse";
export type { CaseHistoryPoint } from "./generated/CaseHistoryPoint";

export type { CacheStatsResponse } from "./generated/CacheStatsResponse";
export type { PruneResponse } from "./generated/PruneResponse";
