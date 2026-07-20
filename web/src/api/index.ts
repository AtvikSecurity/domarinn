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

export type { IngestResponse } from "./generated/IngestResponse";
export type { RunDetailResponse } from "./generated/RunDetailResponse";
export type { RunId } from "./generated/RunId";
export type { RunListItem } from "./generated/RunListItem";
export type { RunListResponse } from "./generated/RunListResponse";
export type { RunStatusFilter } from "./generated/RunStatusFilter";

export type { CompareCaseRow } from "./generated/CompareCaseRow";
export type { CompareDelta } from "./generated/CompareDelta";
export type { CompareResponse } from "./generated/CompareResponse";
export type { CompareSummary } from "./generated/CompareSummary";
export type { BaselineBody } from "./generated/BaselineBody";

export type { ProjectListItem } from "./generated/ProjectListItem";
export type { ProjectsResponse } from "./generated/ProjectsResponse";
export type { SuitePoint } from "./generated/SuitePoint";
export type { SuiteSummary } from "./generated/SuiteSummary";
export type { SuitesResponse } from "./generated/SuitesResponse";

export type { CacheStatsResponse } from "./generated/CacheStatsResponse";
export type { PruneResponse } from "./generated/PruneResponse";
