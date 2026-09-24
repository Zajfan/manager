// Renders the real `Panel` into a simulated DOM and drives it with real
// keyboard and mouse events — closing the gap `panelLogic.test.ts` leaves:
// that the pure math is right doesn't yet prove the component actually
// wires a keypress to it. `invoke` is the only thing mocked; everything
// else — SolidJS's reactivity, the DOM, the event handlers — is real.

import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Panel } from "./Panel";
import type { ListingDto } from "./types";

afterEach(cleanup);

function listing(path: string, names: [string, boolean][]): ListingDto {
  return {
    path,
    parent: path === "file:///" ? null : "file:///",
    entries: names.map(([name, isDir]) => ({
      name,
      path: `${path}${name}`,
      kind: isDir ? "dir" : "file",
      isDirLike: isDir,
      size: 100,
      modifiedMs: null,
      hidden: false,
    })),
  };
}

/** A fake `invoke` that answers `list_dir` from a `path -> ListingDto` map. */
function fakeInvoke(byPath: Record<string, ListingDto>) {
  return vi.fn(async (command: string, args?: Record<string, unknown>) => {
    if (command === "list_dir") {
      const path = args?.path as string;
      const found = byPath[path];
      if (!found) throw new Error(`no fixture for ${path}`);
      return found;
    }
    throw new Error(`unexpected command: ${command}`);
  });
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => (globalThis as any).__invoke(...args),
}));

function setup(byPath: Record<string, ListingDto>) {
  const invoke = fakeInvoke(byPath);
  (globalThis as any).__invoke = invoke;
  return invoke;
}

describe("Panel", () => {
  it("shows the folder it starts on", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["docs", true], ["a.txt", false]]),
    });

    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);

    expect(await screen.findByText("docs")).toBeInTheDocument();
    expect(screen.getByText("a.txt")).toBeInTheDocument();
  });

  it("selects the first row to start, and ArrowDown moves on", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["a.txt", false], ["b.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");

    const rowFor = (name: string) => screen.getByText(name).closest("tr")!;
    expect(rowFor("a.txt").className).toContain("selected");

    fireEvent.keyDown(screen.getByText("a.txt").closest(".panel")!, { key: "ArrowDown" });

    expect(rowFor("b.txt").className).toContain("selected");
    expect(rowFor("a.txt").className).not.toContain("selected");
  });

  it("Enter opens the folder under the cursor and shows its contents", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["docs", true]]),
      "file:///home/docs": listing("file:///home/docs", [["deep.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("docs");

    fireEvent.keyDown(screen.getByText("docs").closest(".panel")!, { key: "Enter" });

    expect(await screen.findByText("deep.txt")).toBeInTheDocument();
  });

  it("Enter on a plain file does nothing — no viewer yet in the shell", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");

    fireEvent.keyDown(screen.getByText("a.txt").closest(".panel")!, { key: "Enter" });

    // Still the same folder: nothing navigated away.
    await waitFor(() => expect(screen.getByText("a.txt")).toBeInTheDocument());
  });

  it("Backspace goes up to the parent, landing back on the folder it left", async () => {
    setup({
      "file:///home/docs": listing("file:///home/docs", [["a.txt", false]]),
      "file:///": listing("file:///", [["home", true]]),
    });
    render(() => (
      <Panel initialPath="file:///home/docs" active={() => true} onActivate={() => {}} />
    ));
    await screen.findByText("a.txt");

    fireEvent.keyDown(screen.getByText("a.txt").closest(".panel")!, { key: "Backspace" });

    expect(await screen.findByText("home")).toBeInTheDocument();
  });

  it("a key is ignored while the panel is not the active one", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["a.txt", false], ["b.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => false} onActivate={() => {}} />);
    await screen.findByText("a.txt");

    fireEvent.keyDown(screen.getByText("a.txt").closest(".panel")!, { key: "ArrowDown" });

    expect(screen.getByText("a.txt").closest("tr")!.className).toContain("selected");
  });

  it("clicking a row selects it and calls onActivate", async () => {
    const onActivate = vi.fn();
    setup({
      "file:///home/": listing("file:///home/", [["a.txt", false], ["b.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => false} onActivate={onActivate} />);
    await screen.findByText("b.txt");

    fireEvent.click(screen.getByText("b.txt"));

    expect(onActivate).toHaveBeenCalled();
    expect(screen.getByText("b.txt").closest("tr")!.className).toContain("selected");
  });

  it("double-clicking a folder opens it", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["docs", true]]),
      "file:///home/docs": listing("file:///home/docs", [["deep.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("docs");

    fireEvent.dblClick(screen.getByText("docs"));

    expect(await screen.findByText("deep.txt")).toBeInTheDocument();
  });

  it("shows an empty folder plainly rather than a blank table", async () => {
    setup({ "file:///home/": listing("file:///home/", []) });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);

    expect(await screen.findByText("(empty)")).toBeInTheDocument();
  });
});
