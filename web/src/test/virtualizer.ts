/**
 * Make `useVirtualizer` render rows under jsdom.
 *
 * jsdom has no layout, so the virtualizer sees a zero-height viewport and
 * renders nothing at all — a grid test without this passes vacuously, finding
 * zero rows and asserting nothing about them. It is why the case grid was
 * covered only by Playwright for so long.
 *
 * The measurement shim has to target `offsetWidth`/`offsetHeight`
 * specifically: virtual-core's `getRect` reads those, not
 * `getBoundingClientRect`, and jsdom hardcodes both to 0. Shimming the wrong
 * one looks correct and changes nothing, which is exactly the failure mode
 * this module exists to stop the next author rediscovering.
 *
 * Call from a `beforeAll` in any test that renders a virtualized grid.
 */
export function installVirtualizerShims(width = 1200, height = 800): void {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;

  for (const [prop, value] of [
    ["offsetWidth", width],
    ["offsetHeight", height],
  ] as const) {
    Object.defineProperty(HTMLElement.prototype, prop, {
      configurable: true,
      get: () => value,
    });
  }
}
