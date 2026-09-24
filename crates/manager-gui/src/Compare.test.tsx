// Drives the real `Compare` component with real key/mouse events, the same
// way Search.test.tsx does — `invoke` and `listen` are the only things
// mocked.

import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Compare } from "./Compare";
import type { DiffEntryDto } from "./types";

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

function setup(syncResult: { id: number; title: string }[] = []) {
  const invoke = vi.fn(async (command: string) => {
    if (command === "start_compare" || command === "cancel_compare") return undefined;
    if (command === "sync_compare") return syncResult;
    throw new Error(`unexpected command: ${command}`);
  });
  (globalThis as any).__invoke = invoke;
  return invoke;
}

function diff(key: string, status: DiffEntryDto["status"]): DiffEntryDto {
  return { key, name: key, left: { name: key, path: `file:///left/${key}`, kind: "file", isDirLike: false, size: 10, modifiedMs: null, hidden: false }, right: null, status, newer: null };
}

function submit(overlay: Element) {
  fireEvent.keyDown(overlay, { key: "Enter" });
}

describe("Compare", () => {
  it("Enter starts comparing with both paths and the dialog's switches", async () => {
    const invoke = setup();
    render(() => <Compare left="file:///left/" right="file:///right/" onClose={() => {}} />);
    const overlay = screen.getByText("Compare folders").closest(".overlay")!;

    fireEvent.keyDown(overlay, { key: "c", altKey: true });
    submit(overlay);

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_compare", {
        left: "file:///left/",
        right: "file:///right/",
        byContent: true,
        includeHidden: false,
      }),
    );
  });

  it("shows differences as they arrive and the running count", async () => {
    setup();
    render(() => <Compare left="file:///left/" right="file:///right/" onClose={() => {}} />);
    submit(screen.getByText("Compare folders").closest(".overlay")!);
    await screen.findByText(/Comparing/);

    emitTauriEvent("compare-found", diff("a.txt", "differs"));
    emitTauriEvent("compare-progress", { scanned: 5, found: 1 });

    expect(await screen.findByText("a.txt")).toBeInTheDocument();
    expect(screen.getByText(/5 scanned, 1 found/)).toBeInTheDocument();
  });

  it("Space marks a syncable entry but not one that's the same on both sides", async () => {
    setup();
    render(() => <Compare left="file:///left/" right="file:///right/" onClose={() => {}} />);
    const overlay = screen.getByText("Compare folders").closest(".overlay")!;
    submit(overlay);
    await screen.findByText(/Comparing/);
    emitTauriEvent("compare-found", diff("a.txt", "same"));
    await screen.findByText("a.txt");

    fireEvent.keyDown(overlay, { key: " " });

    expect(screen.getByText("a.txt").closest("tr")!.className).not.toContain("marked");
  });

  it("> syncs the selection left-to-right and shows the job's progress", async () => {
    const invoke = setup([{ id: 9, title: "Copy 1 item → file:///right/" }]);
    render(() => <Compare left="file:///left/" right="file:///right/" onClose={() => {}} />);
    const overlay = screen.getByText("Compare folders").closest(".overlay")!;
    submit(overlay);
    await screen.findByText(/Comparing/);
    emitTauriEvent("compare-found", diff("a.txt", "leftOnly"));
    await screen.findByText("a.txt");

    fireEvent.keyDown(overlay, { key: ">" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("sync_compare", {
        keys: ["a.txt"],
        direction: "leftToRight",
      }),
    );
    expect(await screen.findByText(/Copy 1 item/)).toBeInTheDocument();
  });

  it("Escape does nothing while a sync job is still running, so its question can still be answered", async () => {
    const invoke = setup([{ id: 9, title: "Copy 1 item → file:///right/" }]);
    const onClose = vi.fn();
    render(() => <Compare left="file:///left/" right="file:///right/" onClose={onClose} />);
    const overlay = screen.getByText("Compare folders").closest(".overlay")!;
    submit(overlay);
    await screen.findByText(/Comparing/);
    emitTauriEvent("compare-found", diff("a.txt", "leftOnly"));
    await screen.findByText("a.txt");
    fireEvent.keyDown(overlay, { key: ">" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("sync_compare", expect.anything()));
    await screen.findByText(/Copy 1 item/);

    fireEvent.keyDown(overlay, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();

    emitTauriEvent("job-finished", { id: 9, title: "Copy 1 item", outcome: "completed" });
    await waitFor(() => expect(screen.queryByText(/Copy 1 item/)).not.toBeInTheDocument());

    fireEvent.keyDown(overlay, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });

  it("O answers a job-conflict raised by a sync job", async () => {
    const invoke = setup([{ id: 9, title: "Copy 1 item → file:///right/" }]);
    render(() => <Compare left="file:///left/" right="file:///right/" onClose={() => {}} />);
    const overlay = screen.getByText("Compare folders").closest(".overlay")!;
    submit(overlay);
    await screen.findByText(/Comparing/);
    emitTauriEvent("compare-found", diff("a.txt", "differs"));
    await screen.findByText("a.txt");
    fireEvent.keyDown(overlay, { key: ">" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("sync_compare", expect.anything()));

    emitTauriEvent("job-conflict", {
      job: 9,
      source: { name: "a.txt", isDirLike: false, size: 5 },
      existing: { name: "a.txt", isDirLike: false, size: 20 },
    });
    expect(await screen.findByText(/already exists/)).toBeInTheDocument();

    fireEvent.keyDown(overlay, { key: "o" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("answer_conflict", {
        job: 9,
        action: "overwrite",
        applyToAll: false,
      }),
    );
  });
});
