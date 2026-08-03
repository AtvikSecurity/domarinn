import { describe, expect, it } from "vitest";
import {
  clampWidth,
  type ColumnDef,
  COLUMNS_KEY,
  cssVarsFor,
  effectiveWidth,
  gridTemplateFor,
  hiddenCount,
  isVisible,
  LEGACY_GRID_COLUMNS_KEY,
  MAX_COLUMN_WIDTH,
  migrateLegacyPrefs,
  minWidthFor,
  parsePrefs,
  serializePrefs,
  type TablePrefs,
  trackFor,
  visibleColumns,
} from "./tableColumns";

const COLS: ColumnDef[] = [
  { id: "status", label: "Status", track: "84px", min: 84, alwaysVisible: true },
  { id: "name", label: "Name", track: "minmax(240px, 1.2fr)", min: 240, alwaysVisible: true },
  { id: "preview", label: "Preview", track: "minmax(160px, 1.4fr)", min: 160 },
  { id: "tokens", label: "Tokens", track: "76px", min: 76, numeric: true },
  { id: "extra", label: "Extra", track: "76px", min: 76, defaultVisible: false },
];

const prefs = (over: Partial<TablePrefs> = {}): TablePrefs => ({
  visible: {},
  width: {},
  ...over,
});

describe("isVisible", () => {
  it("shows a column with no stored opinion", () => {
    expect(isVisible(COLS[2]!, prefs())).toBe(true);
  });

  // The sparse map is load-bearing: only ids the user actually toggled are
  // stored, so a column added in a later release gets its own default instead
  // of inheriting a stale `false` from a blob written before it existed.
  it("honours a column's own default when nothing is stored", () => {
    expect(isVisible(COLS[4]!, prefs())).toBe(false);
  });

  it("honours a stored override in both directions", () => {
    expect(isVisible(COLS[2]!, prefs({ visible: { preview: false } }))).toBe(false);
    expect(isVisible(COLS[4]!, prefs({ visible: { extra: true } }))).toBe(true);
  });

  it("cannot hide a structural column", () => {
    expect(isVisible(COLS[0]!, prefs({ visible: { status: false } }))).toBe(true);
  });
});

describe("visibleColumns", () => {
  it("drops hidden columns and keeps order", () => {
    const out = visibleColumns(COLS, prefs({ visible: { preview: false } }));
    expect(out.map((c) => c.id)).toEqual(["status", "name", "tokens"]);
  });

  // A grid with every column hidden has no way back — the picker that would
  // undo it is anchored to a table that no longer renders.
  it("never returns an empty set, even from a corrupt blob", () => {
    const all = Object.fromEntries(COLS.map((c) => [c.id, false]));
    const out = visibleColumns(
      COLS.map((c) => ({ ...c, alwaysVisible: false })),
      prefs({ visible: all }),
    );
    expect(out.length).toBeGreaterThan(0);
  });
});

describe("hiddenCount", () => {
  it("counts only columns the picker could bring back", () => {
    // `extra` starts hidden, `preview` was hidden by the user; the two
    // alwaysVisible ones are not offered and must not be counted.
    expect(hiddenCount(COLS, prefs({ visible: { preview: false } }))).toBe(2);
  });
});

describe("clampWidth", () => {
  it("refuses to go under the column's floor", () => {
    expect(clampWidth(COLS[3]!, 10)).toBe(76);
  });

  it("caps at the shared ceiling unless the column sets its own", () => {
    expect(clampWidth(COLS[3]!, 99999)).toBe(MAX_COLUMN_WIDTH);
    expect(clampWidth({ ...COLS[3]!, max: 200 }, 99999)).toBe(200);
  });

  it("rounds to whole pixels and rejects nonsense", () => {
    expect(clampWidth(COLS[3]!, 123.6)).toBe(124);
    expect(clampWidth(COLS[3]!, Number.NaN)).toBe(76);
  });
});

describe("trackFor", () => {
  it("uses the column's own track when nothing was resized", () => {
    expect(trackFor(COLS[3]!, prefs())).toBe("76px");
  });

  it("pins a fixed column to the stored width", () => {
    expect(trackFor(COLS[3]!, prefs({ width: { tokens: 120 } }))).toBe("120px");
  });

  // Writing a resized flexible column as a bare `Npx` takes it out of the flex
  // pool for good, and the grid stops filling its container at any viewport —
  // permanently, because the preference persists. Keeping the fr share honours
  // the user's floor without that.
  it("keeps a flexible column flexible after a resize", () => {
    expect(trackFor(COLS[2]!, prefs({ width: { preview: 300 } }))).toBe(
      "minmax(300px, 1.4fr)",
    );
  });

  it("clamps a stored width that is out of range", () => {
    expect(trackFor(COLS[3]!, prefs({ width: { tokens: 5 } }))).toBe("76px");
  });
});

describe("effectiveWidth", () => {
  const col = (track: string, min = 90): ColumnDef => ({
    id: "c",
    label: "C",
    track,
    min,
  });

  it("prefers a stored width, clamped", () => {
    expect(effectiveWidth(col("150px"), prefs({ width: { c: 220 } }))).toBe(220);
    expect(effectiveWidth(col("150px"), prefs({ width: { c: 10 } }))).toBe(90);
  });

  // Falling back to `min` is what makes the first drag of an untouched column
  // jump: the handle starts from the floor rather than from where the column
  // actually is.
  it("falls back to a bare pixel track, not the floor", () => {
    expect(effectiveWidth(col("150px"), prefs())).toBe(150);
  });

  it("falls back to a minmax track's pixel floor", () => {
    expect(effectiveWidth(col("minmax(240px, 1.2fr)", 200), prefs())).toBe(240);
  });

  it("falls back to the column's own floor when the track has no pixels", () => {
    expect(effectiveWidth(col("auto"), prefs())).toBe(90);
    expect(effectiveWidth(col("16%"), prefs())).toBe(90);
    expect(effectiveWidth(col("1.5fr"), prefs())).toBe(90);
  });
});

describe("gridTemplateFor and minWidthFor", () => {
  it("joins the visible tracks in order", () => {
    expect(gridTemplateFor(COLS, prefs({ visible: { preview: false } }))).toBe(
      "84px minmax(240px, 1.2fr) 76px",
    );
  });

  it("sums the visible floors, honouring resizes and the container inset", () => {
    const p = prefs({ visible: { preview: false }, width: { tokens: 120 } });
    // 84 + 240 + 120 + 24
    expect(minWidthFor(COLS, p, 24)).toBe(468);
  });
});

describe("cssVarsFor", () => {
  // One style object on the scroll container beats an inline style per cell:
  // a drag then updates a single node and every row reflows in CSS, with no
  // React re-render of any row.
  it("emits one custom property per visible column", () => {
    const vars = cssVarsFor(COLS, prefs({ visible: { preview: false } }));
    expect(vars["--col-tokens-size"]).toBe("76px");
    expect(vars["--col-preview-size"]).toBeUndefined();
  });
});

describe("parsePrefs", () => {
  it("reads a stored blob", () => {
    const raw = serializePrefs({ cases: { visible: { a: false }, width: { b: 120 } } });
    expect(parsePrefs(raw).cases).toEqual({ visible: { a: false }, width: { b: 120 } });
  });

  it("returns empty for absent storage", () => {
    expect(parsePrefs(null)).toEqual({});
  });

  // A corrupt value must degrade to defaults. Throwing, or returning something
  // that hides columns, would break the table it was meant to configure.
  it("degrades to defaults on junk", () => {
    expect(parsePrefs("not json")).toEqual({});
    expect(parsePrefs("[1,2,3]")).toEqual({});
    expect(parsePrefs('{"cases":"nope"}')).toEqual({});
  });

  it("drops entries of the wrong type rather than the whole table", () => {
    const parsed = parsePrefs(
      '{"cases":{"visible":{"a":false,"b":"yes"},"width":{"c":120,"d":"wide"}}}',
    );
    expect(parsed.cases).toEqual({ visible: { a: false }, width: { c: 120 } });
  });
});

describe("migrateLegacyPrefs", () => {
  it("adopts the old flat visibility map as the case grid's", () => {
    const out = migrateLegacyPrefs({}, '{"assert:contains":true,"cost":false}');
    expect(out.cases).toEqual({
      visible: { "assert:contains": true, cost: false },
      width: {},
    });
  });

  // The new key wins outright: a user who has since configured columns must
  // not have a stale pre-upgrade blob reapplied over the top.
  it("leaves an existing entry alone", () => {
    const existing = { cases: { visible: { cost: true }, width: {} } };
    expect(migrateLegacyPrefs(existing, '{"cost":false}')).toEqual(existing);
  });

  it("ignores junk and absent legacy storage", () => {
    expect(migrateLegacyPrefs({}, null)).toEqual({});
    expect(migrateLegacyPrefs({}, "garbage")).toEqual({});
  });
});

describe("storage keys", () => {
  it("follow the app's convention", () => {
    expect(COLUMNS_KEY).toBe("domarinn.columns");
    expect(LEGACY_GRID_COLUMNS_KEY).toBe("domarinn.grid.columns");
  });
});
