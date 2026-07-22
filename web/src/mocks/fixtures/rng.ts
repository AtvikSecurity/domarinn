// Seeded RNG helpers + numeric rounding shared by every fixture module.

// ---------------------------------------------------------------------------
// Seeded RNG helpers (mulberry32 + a small string/number hash).
// ---------------------------------------------------------------------------

export function hash(...parts: (string | number)[]): number {
  let h = 2166136261 >>> 0;
  const str = parts.join("|");
  for (let i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** Deterministic float in [0, 1) from any set of seed parts. */
export function rand(...parts: (string | number)[]): number {
  let a = hash(...parts);
  a |= 0;
  a = (a + 0x6d2b79f5) | 0;
  let t = Math.imul(a ^ (a >>> 15), 1 | a);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

export function pick<T>(arr: readonly T[], ...seed: (string | number)[]): T {
  // `rand()` is in [0, 1), so `floor(rand() * len)` is always a valid index in
  // [0, len - 1] for any non-empty array; every call site passes a non-empty
  // constant pool. The assertion encodes that proven in-bounds invariant.
  return arr[Math.floor(rand(...seed) * arr.length)]!;
}

export function clamp(n: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, n));
}

/** Epoch millis -> RFC3339, matching the server's wire format for every
 *  timestamp field (see `crates/domarinn-server/src/dto/accounts.rs::rfc3339`
 *  for the server-side equivalent). */
export function toIso(ms: number): string {
  return new Date(ms).toISOString();
}

export function round2(n: number): number {
  return Math.round(n * 100) / 100;
}
export function round4(n: number): number {
  return Math.round(n * 10000) / 10000;
}
