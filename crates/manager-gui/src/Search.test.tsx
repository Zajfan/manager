// Drives the real `Search` component with real key/mouse events, the same
// way Panel.test.tsx does — `invoke` and `listen` are the only things
// mocked.

import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Search } from "./Search";

afterEach(() => {
  cleanup();
  clearListeners();
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => (globalThis as any).__invoke(...args),
}));

const { registerListener, emitTauriEvent, clearListeners } = vi.hoisted(() => {
  let handlers: Record<string, ((event: { payload: unknown }) => void)[]> = {};
  return {
    registerListener(name: string, handler: (event: { payload: unknown }) => void) {
      (handlers[name] ??= []).push(handler);
      return Promise.resolve(() => {
        handlers[name] = (handlers[name] ?? []).filter((h) => h !== handler);
      });
    },
    emitTauriEvent(name: string, payload: unknown) {
      for (const handler of handlers[name] ?? []) handler({ payload });
    },
    clearListeners() {
      handlers = {};
    },
  };
});

vi.mock("@tauri-apps/api/event", () => ({
  listen: registerListener,
}));

function setup() {
  const invoke = vi.fn(async (command: string) => {
    if (command === "start_search" || command === "cancel_search") return undefined;
    throw new Error(`unexpected command: ${command}`);
  });
  (globalThis as any).__invoke = invoke;
  return invoke;
}

function entry(path: string, size = 100) {
  return { name: path.split("/").pop(), path, kind: "file", isDirLike: false, size, modifiedMs: null, hidden: false };
}

describe("Search", () => {
  it("starts with * as the default mask and an empty Containing field", async () => {
    setup();
    render(() => <Search rootPath="file:///home/" onClose={() => {}} onGoto={() => {}} />);

    const [mask, text] = screen.getAllByRole("textbox") as HTMLInputElement[];
    expect(mask.value).toBe("*");
    expect(text.value).toBe("");
  });

  it("Enter submits and calls start_search with the typed fields", async () => {
    const invoke = setup();
    render(() => <Search rootPath="file:///home/" onClose={() => {}} onGoto={() => {}} />);
    const overlay = screen.getByText("Find files").closest(".overlay")!;
    const [mask, text] = screen.getAllByRole("textbox") as HTMLInputElement[];

    fireEvent.input(mask, { target: { value: "*.rs" } });
    fireEvent.input(text, { target: { value: "TODO" } });
    fireEvent.keyDown(overlay, { key: "Enter" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_search", {
        root: "file:///home/",
        mask: "*.rs",
        text: "TODO",
        caseSensitive: false,
        includeHidden: false,
      }),
    );
    expect(screen.getByText(/\*\.rs containing "TODO" in file:\/\/\/home\//)).toBeInTheDocument();
  });

  it("Alt+C and Alt+H toggle the switches before submitting", async () => {
    const invoke = setup();
    render(() => <Search rootPath="file:///home/" onClose={() => {}} onGoto={() => {}} />);
    const overlay = screen.getByText("Find files").closest(".overlay")!;

    fireEvent.keyDown(overlay, { key: "c", altKey: true });
    fireEvent.keyDown(overlay, { key: "h", altKey: true });
    fireEvent.keyDown(overlay, { key: "Enter" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "start_search",
        expect.objectContaining({ caseSensitive: true, includeHidden: true }),
      ),
    );
  });

  it("Escape on the dialog closes without searching", async () => {
    const invoke = setup();
    const onClose = vi.fn();
    render(() => <Search rootPath="file:///home/" onClose={onClose} onGoto={() => {}} />);
    const overlay = screen.getByText("Find files").closest(".overlay")!;

    fireEvent.keyDown(overlay, { key: "Escape" });

    expect(onClose).toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith("start_search", expect.anything());
  });

  it("shows hits as they arrive, and the count while still running", async () => {
    setup();
    render(() => <Search rootPath="file:///home/" onClose={() => {}} onGoto={() => {}} />);
    fireEvent.keyDown(screen.getByText("Find files").closest(".overlay")!, { key: "Enter" });
    await screen.findByText(/Searching/);

    emitTauriEvent("search-found", entry("file:///home/a.rs"));
    emitTauriEvent("search-progress", { scanned: 12, found: 1 });

    expect(await screen.findByText("file:///home/a.rs")).toBeInTheDocument();
    expect(screen.getByText(/12 scanned, 1 found/)).toBeInTheDocument();
  });

  it("a search-finished event switches to the done summary", async () => {
    setup();
    render(() => <Search rootPath="file:///home/" onClose={() => {}} onGoto={() => {}} />);
    fireEvent.keyDown(screen.getByText("Find files").closest(".overlay")!, { key: "Enter" });
    await screen.findByText(/Searching/);

    emitTauriEvent("search-found", entry("file:///home/a.rs"));
    emitTauriEvent("search-finished", { found: 1, scanned: 20, unreadable: 0, cancelled: false });

    expect(await screen.findByText(/Done — 1 found, 20 scanned/)).toBeInTheDocument();
  });

  it("Enter on a hit goes there and Escape closes without going anywhere", async () => {
    setup();
    const onGoto = vi.fn();
    const onClose = vi.fn();
    render(() => <Search rootPath="file:///home/" onClose={onClose} onGoto={onGoto} />);
    const overlay = screen.getByText("Find files").closest(".overlay")!;
    fireEvent.keyDown(overlay, { key: "Enter" });
    await screen.findByText(/Searching/);
    emitTauriEvent("search-found", entry("file:///home/a.rs"));
    await screen.findByText("file:///home/a.rs");

    fireEvent.keyDown(overlay, { key: "Enter" });
    expect(onGoto).toHaveBeenCalledWith("file:///home/a.rs");
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.keyDown(overlay, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });

  it("cancels the search on unmount so nothing keeps running unseen", async () => {
    const invoke = setup();
    const { unmount } = render(() => (
      <Search rootPath="file:///home/" onClose={() => {}} onGoto={() => {}} />
    ));
    fireEvent.keyDown(screen.getByText("Find files").closest(".overlay")!, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_search", expect.anything()));

    unmount();

    expect(invoke.mock.calls.some(([cmd]) => cmd === "cancel_search")).toBe(true);
  });
});
