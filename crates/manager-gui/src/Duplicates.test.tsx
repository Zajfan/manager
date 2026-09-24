// Drives the real `Duplicates` component with real key/mouse events, the
// same way Search.test.tsx and Compare.test.tsx do — `invoke` and `listen`
// are the only things mocked.

import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Duplicates } from "./Duplicates";
import type { DuplicateGroupDto } from "./types";

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

function setup(deleteResult = { id: 9, title: "Trash 2 items" }) {
  const invoke = vi.fn(async (command: string) => {
    if (command === "start_duplicates" || command === "cancel_duplicates") return undefined;
    if (command === "start_delete") return deleteResult;
    throw new Error(`unexpected command: ${command}`);
  });
  (globalThis as any).__invoke = invoke;
  return invoke;
}

function group(size: number, ...paths: string[]): DuplicateGroupDto {
  return {
    size,
    files: paths.map((p) => ({
      name: p.split("/").pop()!,
      path: p,
      kind: "file" as const,
      isDirLike: false,
      size,
      modifiedMs: null,
      hidden: false,
    })),
  };
}

function submit(overlay: Element) {
  fireEvent.keyDown(overlay, { key: "Enter" });
}

describe("Duplicates", () => {
  it("Enter starts the search rooted at rootPath with the typed mask", async () => {
    const invoke = setup();
    render(() => <Duplicates rootPath="file:///home/" onClose={() => {}} />);
    const overlay = screen.getByText("Find duplicate files").closest(".overlay")!;
    const mask = screen.getByRole("textbox") as HTMLInputElement;

    fireEvent.input(mask, { target: { value: "*.rs" } });
    submit(overlay);

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_duplicates", {
        root: "file:///home/",
        mask: "*.rs",
        includeHidden: false,
      }),
    );
  });

  it("shows groups as they arrive, with a heading row for each", async () => {
    setup();
    render(() => <Duplicates rootPath="file:///home/" onClose={() => {}} />);
    submit(screen.getByText("Find duplicate files").closest(".overlay")!);
    await screen.findByText(/Looking for duplicates/);

    emitTauriEvent("compare-progress", {});
    emitTauriEvent("duplicates-found", group(100, "/a.txt", "/b.txt"));

    expect(await screen.findByText("/a.txt")).toBeInTheDocument();
    expect(screen.getByText("/b.txt")).toBeInTheDocument();
    expect(screen.getByText(/100 B each/)).toBeInTheDocument();
  });

  it("K marks every file but the first of each group", async () => {
    setup();
    render(() => <Duplicates rootPath="file:///home/" onClose={() => {}} />);
    const overlay = screen.getByText("Find duplicate files").closest(".overlay")!;
    submit(overlay);
    await screen.findByText(/Looking for duplicates/);
    emitTauriEvent("duplicates-found", group(10, "/a", "/b", "/c"));
    await screen.findByText("/a");

    fireEvent.keyDown(overlay, { key: "K" });

    expect(screen.getByText("/a").closest("tr")!.className).not.toContain("marked");
    expect(screen.getByText("/b").closest("tr")!.className).toContain("marked");
    expect(screen.getByText("/c").closest("tr")!.className).toContain("marked");
  });

  it("D asks to confirm, and Enter deletes the marked files", async () => {
    const invoke = setup();
    render(() => <Duplicates rootPath="file:///home/" onClose={() => {}} />);
    const overlay = screen.getByText("Find duplicate files").closest(".overlay")!;
    submit(overlay);
    await screen.findByText(/Looking for duplicates/);
    emitTauriEvent("duplicates-found", group(10, "/a", "/b"));
    await screen.findByText("/a");
    fireEvent.keyDown(overlay, { key: "K" }); // marks /b

    fireEvent.keyDown(overlay, { key: "d" });
    expect(await screen.findByText(/Move 1 item/)).toBeInTheDocument();

    fireEvent.keyDown(overlay, { key: "Enter" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_delete", { targets: ["/b"], permanent: false }),
    );
    expect(await screen.findByText(/Trash 2 items/)).toBeInTheDocument();
  });

  it("Escape does nothing while the delete job is still running", async () => {
    const invoke = setup();
    const onClose = vi.fn();
    render(() => <Duplicates rootPath="file:///home/" onClose={onClose} />);
    const overlay = screen.getByText("Find duplicate files").closest(".overlay")!;
    submit(overlay);
    await screen.findByText(/Looking for duplicates/);
    emitTauriEvent("duplicates-found", group(10, "/a"));
    await screen.findByText("/a");
    fireEvent.keyDown(overlay, { key: "ArrowDown" }); // off the group heading, onto the file
    fireEvent.keyDown(overlay, { key: "d" });
    fireEvent.keyDown(overlay, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_delete", expect.anything()));

    fireEvent.keyDown(overlay, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();

    emitTauriEvent("job-finished", { id: 9, title: "Trash 2 items", outcome: "completed" });
    await waitFor(() => expect(screen.queryByText(/Trash 2 items/)).not.toBeInTheDocument());

    fireEvent.keyDown(overlay, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });

  it("a job-error from the delete is answered by S, R, A or C", async () => {
    const invoke = setup();
    render(() => <Duplicates rootPath="file:///home/" onClose={() => {}} />);
    const overlay = screen.getByText("Find duplicate files").closest(".overlay")!;
    submit(overlay);
    await screen.findByText(/Looking for duplicates/);
    emitTauriEvent("duplicates-found", group(10, "/a"));
    await screen.findByText("/a");
    fireEvent.keyDown(overlay, { key: "ArrowDown" }); // off the group heading, onto the file
    fireEvent.keyDown(overlay, { key: "d" });
    fireEvent.keyDown(overlay, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_delete", expect.anything()));

    emitTauriEvent("job-error", { job: 9, message: "permission denied" });
    expect(await screen.findByText(/permission denied/)).toBeInTheDocument();

    fireEvent.keyDown(overlay, { key: "s" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("answer_error", { job: 9, answer: "skip" }),
    );
  });
});
