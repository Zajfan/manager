import { createRoot } from "solid-js";
import { describe, expect, it } from "vitest";
import { createOverlayState } from "./overlayState";

describe("createOverlayState", () => {
  it("routes a goto back to the panel that opened the search, and closes the search", () => {
    createRoot((dispose) => {
      const overlay = createOverlayState<0 | 1>();
      overlay.openSearch(1, "file:///home/");

      overlay.handleGoto("file:///home/found.txt");

      expect(overlay.gotoTargetFor(1)()).toBe("file:///home/found.txt");
      expect(overlay.gotoTargetFor(0)()).toBeNull();
      expect(overlay.search()).toBeNull();
      dispose();
    });
  });

  it("clearing one panel's goto target leaves the other panel's alone", () => {
    createRoot((dispose) => {
      const overlay = createOverlayState<0 | 1>();
      overlay.openSearch(0, "file:///a/");
      overlay.handleGoto("file:///a/x.txt");
      overlay.openSearch(1, "file:///b/");
      overlay.handleGoto("file:///b/y.txt");

      overlay.clearGotoTarget(0);

      expect(overlay.gotoTargetFor(0)()).toBeNull();
      expect(overlay.gotoTargetFor(1)()).toBe("file:///b/y.txt");
      dispose();
    });
  });

  it("works with a single non-numeric panel id, for a one-pane shell", () => {
    createRoot((dispose) => {
      const overlay = createOverlayState<"main">();
      overlay.openSearch("main", "file:///home/");

      overlay.handleGoto("file:///home/x.txt");

      expect(overlay.gotoTargetFor("main")()).toBe("file:///home/x.txt");
      dispose();
    });
  });

  it("handleGoto does nothing if no search is open", () => {
    createRoot((dispose) => {
      const overlay = createOverlayState<0 | 1>();

      overlay.handleGoto("file:///wherever.txt");

      expect(overlay.gotoTargetFor(0)()).toBeNull();
      expect(overlay.gotoTargetFor(1)()).toBeNull();
      dispose();
    });
  });

  it("tracks the duplicate finder's root, independent of any panel id", () => {
    createRoot((dispose) => {
      const overlay = createOverlayState<0 | 1>();
      expect(overlay.duplicatesRoot()).toBeNull();

      overlay.openDuplicates("file:///home/");
      expect(overlay.duplicatesRoot()).toBe("file:///home/");

      overlay.closeDuplicates();
      expect(overlay.duplicatesRoot()).toBeNull();
      dispose();
    });
  });

  it("compareOpen starts closed and toggles via setCompareOpen", () => {
    createRoot((dispose) => {
      const overlay = createOverlayState<0 | 1>();
      expect(overlay.compareOpen()).toBe(false);

      overlay.setCompareOpen(true);

      expect(overlay.compareOpen()).toBe(true);
      dispose();
    });
  });
});
