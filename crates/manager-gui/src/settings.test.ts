// crates/manager-gui/src/settings.test.ts
// getLayout() carries real, spec-mandated fallback logic — not just a
// passthrough to the store — so it gets its own coverage: a store read
// that throws (corrupt file, missing file, plugin error) and a stored
// value that isn't a recognized layout id must both fall back to the
// default layout rather than surfacing an error or an invalid id, and the
// normal first-launch case (nothing stored yet) must land on the default
// too. `LazyStore` is the only thing mocked; `getLayout` itself is real.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { getLayout } from "./settings";
import { DEFAULT_LAYOUT } from "./layouts";

/** `vi.mock` factories can't close over outer variables, so the mocked
 * `get` implementation is created through `vi.hoisted` and shared by both
 * the mock's `LazyStore` class and the tests below that configure it. */
const { mockGet } = vi.hoisted(() => ({
  mockGet: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-store", () => ({
  LazyStore: class {
    get = mockGet;
    set = vi.fn();
    save = vi.fn();
  },
}));

beforeEach(() => {
  mockGet.mockReset();
});

describe("getLayout", () => {
  it("falls back to the default layout when the store read throws", async () => {
    mockGet.mockRejectedValueOnce(new Error("corrupt settings file"));

    await expect(getLayout()).resolves.toBe(DEFAULT_LAYOUT);
  });

  it("falls back to the default layout when the stored value isn't a recognized layout id", async () => {
    mockGet.mockResolvedValueOnce("not-a-real-layout");

    await expect(getLayout()).resolves.toBe(DEFAULT_LAYOUT);
  });

  it("falls back to the default layout when nothing has been stored yet", async () => {
    mockGet.mockResolvedValueOnce(undefined);

    await expect(getLayout()).resolves.toBe(DEFAULT_LAYOUT);
  });
});
