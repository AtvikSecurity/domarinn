// Deterministic in-memory fixture dataset for demo + tests. No randomness that
// changes between reloads: everything is derived from stable seeds so the 500
// case money page, compare deltas, and sparklines are reproducible.
//
// Every function here returns (or is projected into, right before the mock
// handler serializes it) the exact wire shape of the matching generated
// response type — imported from `@/api` so tsc enforces fixture correctness
// against the real server contract, not the other way around.

export type { RunMeta } from "./fixtures/runMeta";
export type { MockCaseRow } from "./fixtures/cases";
export { toCaseListItem } from "./fixtures/cases";
export { configSnapshot, runConfig } from "./fixtures/config";
export {
  allRunSummaries,
  runCases,
  runDetail,
  runListItem,
} from "./fixtures/runStats";
export { buildMatrix } from "./fixtures/matrix";
export { caseDetail } from "./fixtures/caseDetail";
export { searchFixtures } from "./fixtures/search";
export {
  caseHistory,
  defaultCompareTarget,
  setSuiteBaseline,
  suiteBaseline,
} from "./fixtures/history";
export { compareRuns } from "./fixtures/compare";
export {
  cacheEntryDetail,
  cacheEntryList,
  cacheEntryRuns,
  cacheFacets,
} from "./fixtures/cacheEntries";
export {
  cacheStats,
  META,
  projectSummaries,
  suiteSummaries,
} from "./fixtures/summaries";
