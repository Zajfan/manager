// Renders the real `Panel` into a simulated DOM and drives it with real
// keyboard and mouse events — closing the gap `panelLogic.test.ts` leaves:
// that the pure math is right doesn't yet prove the component actually
// wires a keypress to it. `invoke` is the only thing mocked; everything
// else — SolidJS's reactivity, the DOM, the event handlers — is real.

import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Panel } from "./Panel";
import type { ListingDto } from "./types";

afterEach(() => {
  cleanup();
  clearListeners();
});

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

/** A fake `invoke` that answers `list_dir` from a `path -> ListingDto` map,
 * and `start_delete` / `job_action` with a fixed job id so tests can drive
 * the rest of the flow through fake `job-progress` / `job-finished` events. */
function fakeInvoke(byPath: Record<string, ListingDto>) {
  return vi.fn(async (command: string, args?: Record<string, unknown>) => {
    if (command === "list_dir") {
      const path = args?.path as string;
      const found = byPath[path];
      if (!found) throw new Error(`no fixture for ${path}`);
      return found;
    }
    if (command === "start_delete") {
      return { id: 1, title: `Delete ${(args?.targets as string[]).length} items` };
    }
    if (command === "start_transfer") {
      const verb = args?.isMove ? "Move" : "Copy";
      return { id: 1, title: `${verb} ${(args?.sources as string[]).length} items` };
    }
    if (command === "job_action" || command === "answer_conflict" || command === "answer_error") {
      return undefined;
    }
    throw new Error(`unexpected command: ${command}`);
  });
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => (globalThis as any).__invoke(...args),
}));

/** `vi.mock` factories can't close over outer variables, so the handler
 * registry itself is created through `vi.hoisted` and shared by both the
 * mock's `listen` and the `emitTauriEvent` helper tests fire below. */
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

  it("Space marks the row under the cursor and moves on", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["a.txt", false], ["b.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const rowFor = (name: string) => screen.getByText(name).closest("tr")!;

    fireEvent.keyDown(rowFor("a.txt").closest(".panel")!, { key: " " });

    expect(rowFor("a.txt").className).toContain("marked");
    expect(rowFor("b.txt").className).toContain("selected");
  });

  it("Insert on an already-marked row unmarks it", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["a.txt", false], ["b.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const rowFor = (name: string) => screen.getByText(name).closest("tr")!;
    const panel = rowFor("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "Insert" }); // marks a.txt, moves to b.txt
    fireEvent.keyDown(panel, { key: "ArrowUp" }); // back to a.txt
    fireEvent.keyDown(panel, { key: "Insert" }); // unmarks a.txt

    expect(rowFor("a.txt").className).not.toContain("marked");
  });

  it("marking survives moving the cursor, but not navigating into a folder", async () => {
    setup({
      "file:///": listing("file:///", [["a.txt", false], ["docs", true]]),
      "file:///docs": listing("file:///docs", [["deep.txt", false]]),
    });
    render(() => <Panel initialPath="file:///" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: " " }); // marks a.txt, moves to docs
    expect(screen.getByText("a.txt").closest("tr")!.className).toContain("marked");

    fireEvent.keyDown(panel, { key: "Enter" }); // opens docs
    await screen.findByText("deep.txt");
    fireEvent.keyDown(panel, { key: "Backspace" }); // back to the root

    expect(await screen.findByText("a.txt")).toBeInTheDocument();
    expect(screen.getByText("a.txt").closest("tr")!.className).not.toContain("marked");
  });

  it("Delete asks for confirmation naming the marked items, and Escape cancels it", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["a.txt", false], ["b.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: " " }); // marks a.txt
    fireEvent.keyDown(panel, { key: "Delete" });

    expect(await screen.findByText(/Delete 1 item\?/)).toBeInTheDocument();

    fireEvent.keyDown(panel, { key: "Escape" });

    await waitFor(() => expect(screen.queryByText(/Delete 1 item\?/)).not.toBeInTheDocument());
  });

  it("Delete with nothing marked falls back to the entry under the cursor", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["a.txt", false], ["b.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "ArrowDown" }); // cursor on b.txt
    fireEvent.keyDown(panel, { key: "Delete" });

    expect(await screen.findByText(/Delete 1 item\?/)).toBeInTheDocument();
  });

  it("Shift+Delete asks to delete permanently", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "Delete", shiftKey: true });

    expect(await screen.findByText(/Delete permanently 1 item\?/)).toBeInTheDocument();
  });

  it("confirming a delete calls start_delete with the marked paths, then shows progress from job-progress events", async () => {
    const invoke = setup({
      "file:///home/": listing("file:///home/", [["a.txt", false], ["b.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: " " }); // marks a.txt
    fireEvent.keyDown(panel, { key: "Delete" });
    fireEvent.keyDown(panel, { key: "Enter" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_delete", {
        targets: ["file:///home/a.txt"],
        permanent: false,
      }),
    );
    // The mark clears as soon as the job is handed off, same as the terminal version.
    expect(screen.getByText("a.txt").closest("tr")!.className).not.toContain("marked");

    emitTauriEvent("job-progress", { id: 1, progress: { fraction: 0.5 } });

    expect(await screen.findByText(/50%/)).toBeInTheDocument();
  });

  it("a job-finished event for this panel's job clears the status and reloads the folder", async () => {
    const invoke = setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "Delete" });
    fireEvent.keyDown(panel, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_delete", expect.anything()));

    emitTauriEvent("job-finished", { id: 1, title: "Delete 1 item", outcome: "completed" });

    await waitFor(() => expect(screen.queryByText(/Delete 1 item/)).not.toBeInTheDocument());
    // Refetched the same folder — list_dir called again for file:///home/.
    const listDirCalls = invoke.mock.calls.filter(([cmd]) => cmd === "list_dir");
    expect(listDirCalls.length).toBeGreaterThanOrEqual(2);
  });

  it("Escape while a job is running cancels it", async () => {
    const invoke = setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "Delete" });
    fireEvent.keyDown(panel, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_delete", expect.anything()));

    fireEvent.keyDown(panel, { key: "Escape" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("job_action", { id: 1, action: "cancel" }),
    );
  });

  it("a job-conflict event shows the question, and O answers overwrite with apply-to-all off", async () => {
    const invoke = setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "Delete" });
    fireEvent.keyDown(panel, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_delete", expect.anything()));

    emitTauriEvent("job-conflict", {
      job: 1,
      source: { name: "a.txt", isDirLike: false, size: 5 },
      existing: { name: "a.txt", isDirLike: false, size: 20 },
    });

    expect(await screen.findByText(/"a\.txt" already exists/)).toBeInTheDocument();
    // The plain job-progress status is replaced while a question is open.
    expect(screen.queryByText(/Delete 1 item/)).not.toBeInTheDocument();

    fireEvent.keyDown(panel, { key: "o" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("answer_conflict", {
        job: 1,
        action: "overwrite",
        applyToAll: false,
      }),
    );
    expect(screen.queryByText(/already exists/)).not.toBeInTheDocument();
  });

  it("Shift+U answers a conflict with overwriteOlder and apply-to-all on", async () => {
    const invoke = setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "Delete" });
    fireEvent.keyDown(panel, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_delete", expect.anything()));
    emitTauriEvent("job-conflict", {
      job: 1,
      source: { name: "a.txt", isDirLike: false, size: 5 },
      existing: { name: "a.txt", isDirLike: false, size: 20 },
    });
    await screen.findByText(/already exists/);

    fireEvent.keyDown(panel, { key: "U", shiftKey: true });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("answer_conflict", {
        job: 1,
        action: "overwriteOlder",
        applyToAll: true,
      }),
    );
  });

  it("a job-error event shows the message, and S answers skip", async () => {
    const invoke = setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "Delete" });
    fireEvent.keyDown(panel, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_delete", expect.anything()));

    emitTauriEvent("job-error", { job: 1, message: "permission denied: a.txt" });

    expect(await screen.findByText(/permission denied: a\.txt/)).toBeInTheDocument();

    fireEvent.keyDown(panel, { key: "s" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("answer_error", { job: 1, answer: "skip" }),
    );
  });

  it("Escape on a pending question cancels it, same as pressing C", async () => {
    const invoke = setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "Delete" });
    fireEvent.keyDown(panel, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_delete", expect.anything()));
    emitTauriEvent("job-error", { job: 1, message: "permission denied" });
    await screen.findByText(/permission denied/);

    fireEvent.keyDown(panel, { key: "Escape" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("answer_error", { job: 1, answer: "cancel" }),
    );
  });

  it("F5 opens a transfer prompt pre-filled with the other panel's path", async () => {
    setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => (
      <Panel
        initialPath="file:///home/"
        active={() => true}
        onActivate={() => {}}
        otherPath={() => "file:///backup/"}
      />
    ));
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "F5" });

    expect(await screen.findByText(/Copy 1 item/)).toBeInTheDocument();
    const input = screen.getByRole("textbox") as HTMLInputElement;
    expect(input.value).toBe("file:///backup/");
  });

  it("Escape closes the transfer prompt without starting anything", async () => {
    const invoke = setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "F6" });
    expect(await screen.findByText(/Move 1 item/)).toBeInTheDocument();

    fireEvent.keyDown(panel, { key: "Escape" });

    await waitFor(() => expect(screen.queryByText(/Move 1 item/)).not.toBeInTheDocument());
    expect(invoke).not.toHaveBeenCalledWith("start_transfer", expect.anything());
  });

  it("editing the destination and pressing Enter starts the transfer with this panel as the base", async () => {
    const invoke = setup({
      "file:///home/": listing("file:///home/", [["a.txt", false], ["b.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: " " }); // marks a.txt
    fireEvent.keyDown(panel, { key: "F5" }); // copy

    const input = screen.getByRole("textbox") as HTMLInputElement;
    fireEvent.input(input, { target: { value: "../backup" } });
    fireEvent.keyDown(panel, { key: "Enter" });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_transfer", {
        sources: ["file:///home/a.txt"],
        base: "file:///home/",
        dest: "../backup",
        isMove: false,
      }),
    );
    expect(screen.getByText("a.txt").closest("tr")!.className).not.toContain("marked");
  });

  it("F6 with nothing marked moves the entry under the cursor", async () => {
    const invoke = setup({
      "file:///home/": listing("file:///home/", [["a.txt", false]]),
    });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);
    await screen.findByText("a.txt");
    const panel = screen.getByText("a.txt").closest(".panel")!;

    fireEvent.keyDown(panel, { key: "F6" });
    fireEvent.keyDown(panel, { key: "Enter" }); // dest left as the pre-filled (empty) value

    // Nothing typed and no otherPath supplied means an empty destination —
    // submitting should be a no-op, not a call with a blank path.
    await waitFor(() => expect(screen.getByText(/Move 1 item/)).toBeInTheDocument());
    expect(invoke).not.toHaveBeenCalledWith("start_transfer", expect.anything());
  });

  it("reports its own path changes so the other panel can default to it", async () => {
    const onPathChange = vi.fn();
    setup({
      "file:///home/": listing("file:///home/", [["docs", true]]),
      "file:///home/docs": listing("file:///home/docs", [["deep.txt", false]]),
    });
    render(() => (
      <Panel
        initialPath="file:///home/"
        active={() => true}
        onActivate={() => {}}
        onPathChange={onPathChange}
      />
    ));
    await screen.findByText("docs");
    expect(onPathChange).toHaveBeenCalledWith("file:///home/");

    fireEvent.keyDown(screen.getByText("docs").closest(".panel")!, { key: "Enter" });

    await waitFor(() => expect(onPathChange).toHaveBeenCalledWith("file:///home/docs"));
  });

  it("shows an empty folder plainly rather than a blank table", async () => {
    setup({ "file:///home/": listing("file:///home/", []) });
    render(() => <Panel initialPath="file:///home/" active={() => true} onActivate={() => {}} />);

    expect(await screen.findByText("(empty)")).toBeInTheDocument();
  });
});
