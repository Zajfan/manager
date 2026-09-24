// One pane: a folder's contents, its own cursor, and the keys that move
// around and open things. Like `manager-tui`'s `Panel`, this holds no
// opinion about sorting or filtering — it just shows what `list_dir` (Rust,
// the same code the terminal version calls) already decided.

import { For, Show, createEffect, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import {
  answerConflict,
  answerError,
  cancelJob,
  onJobConflict,
  onJobError,
  onJobFinished,
  onJobProgress,
  startDelete,
  startTransfer,
  type ConflictAction,
  type ErrorAction,
} from "./jobs";
import {
  clampCursor,
  entryToOpen,
  moveCursor as moveCursorTo,
  selection,
  toggleMark,
} from "./panelLogic";
import type { ConflictQuestionDto, ErrorQuestionDto, EntryDto, ListingDto, ProgressDto } from "./types";
import { formatDate, formatSize } from "./format";

interface ActiveJob {
  id: number;
  title: string;
  progress?: ProgressDto;
  question?:
    | { kind: "conflict"; data: ConflictQuestionDto }
    | { kind: "error"; data: ErrorQuestionDto };
}

interface PendingDelete {
  targets: EntryDto[];
  permanent: boolean;
}

interface PendingTransfer {
  sources: EntryDto[];
  isMove: boolean;
}

export interface PanelProps {
  initialPath: string;
  active: () => boolean;
  onActivate: () => void;
  /** Called whenever this panel navigates, so the other one can default
   * F5/F6's destination to it. Optional only so tests that don't care about
   * cross-panel wiring can skip it. */
  onPathChange?: (path: string) => void;
  /** The other panel's current path, pre-filled as F5/F6's destination —
   * same convention as `manager-tui`'s transfer dialog. */
  otherPath?: () => string;
}

async function fetchListing(path: string): Promise<ListingDto> {
  return invoke<ListingDto>("list_dir", {
    path,
    sortKey: "name",
    reverse: false,
    showHidden: false,
  });
}

export function Panel(props: PanelProps) {
  const [path, setPath] = createSignal(props.initialPath);
  const [cursor, setCursor] = createSignal(0);
  const [marked, setMarked] = createSignal<Set<string>>(new Set());
  const [confirmDelete, setConfirmDelete] = createSignal<PendingDelete | null>(null);
  const [pendingTransfer, setPendingTransfer] = createSignal<PendingTransfer | null>(null);
  const [destInput, setDestInput] = createSignal("");
  const [activeJob, setActiveJob] = createSignal<ActiveJob | null>(null);
  const [listing, { refetch }] = createResource(path, fetchListing);

  createEffect(() => props.onPathChange?.(path()));

  let destInputEl: HTMLInputElement | undefined;
  createEffect(() => {
    if (pendingTransfer()) destInputEl?.focus();
  });

  onMount(() => {
    const progress = onJobProgress((event) => {
      setActiveJob((job) => (job && job.id === event.id ? { ...job, progress: event.progress } : job));
    });
    const finished = onJobFinished((report) => {
      if (activeJob()?.id !== report.id) return;
      setActiveJob(null);
      refetch();
    });
    const conflict = onJobConflict((data) => {
      setActiveJob((job) => (job && job.id === data.job ? { ...job, question: { kind: "conflict", data } } : job));
    });
    const error = onJobError((data) => {
      setActiveJob((job) => (job && job.id === data.job ? { ...job, question: { kind: "error", data } } : job));
    });
    onCleanup(() => {
      void progress.then((unlisten) => unlisten());
      void finished.then((unlisten) => unlisten());
      void conflict.then((unlisten) => unlisten());
      void error.then((unlisten) => unlisten());
    });
  });

  let container: HTMLDivElement | undefined;
  createEffect(() => {
    if (props.active()) container?.focus();
  });

  function entries(): EntryDto[] {
    return listing()?.entries ?? [];
  }

  // Keeps the cursor on a real row whenever the listing changes underneath
  // it — a shorter folder after navigating, or one that came back empty.
  createEffect(() => {
    setCursor((c) => clampCursor(c, entries().length));
  });

  function moveCursor(delta: number) {
    setCursor((c) => moveCursorTo(c, delta, entries().length));
  }

  function openCursor() {
    const entry = entryToOpen(entries(), cursor());
    if (entry) {
      setPath(entry.path);
      setCursor(0);
      setMarked(new Set<string>());
    }
  }

  function goUp() {
    const parent = listing()?.parent;
    if (parent) {
      setPath(parent);
      setCursor(0);
      setMarked(new Set<string>());
    }
  }

  function toggleMarkAtCursor() {
    const entry = entries()[cursor()];
    if (entry) setMarked((m) => toggleMark(m, entry.name));
    moveCursor(1);
  }

  function askToDelete(permanent: boolean) {
    const targets = selection(entries(), marked(), cursor());
    if (targets.length > 0) setConfirmDelete({ targets, permanent });
  }

  async function runDelete(pending: PendingDelete) {
    setConfirmDelete(null);
    setMarked(new Set<string>());
    const started = await startDelete(
      pending.targets.map((e) => e.path),
      pending.permanent,
    );
    setActiveJob({ id: started.id, title: started.title });
  }

  function askToTransfer(isMove: boolean) {
    const sources = selection(entries(), marked(), cursor());
    if (sources.length === 0) return;
    setDestInput(props.otherPath?.() ?? "");
    setPendingTransfer({ sources, isMove });
  }

  async function runTransfer(pending: PendingTransfer, dest: string) {
    if (dest.trim() === "") return;
    setPendingTransfer(null);
    setMarked(new Set<string>());
    const started = await startTransfer(
      pending.sources.map((e) => e.path),
      path(),
      dest,
      pending.isMove,
    );
    setActiveJob({ id: started.id, title: started.title });
  }

  /** Conflict: o/u/s/r (Shift = apply to every later conflict in this job),
   * c/Esc cancels. Error: r/s/a, c/Esc cancels. Mirrors `manager-tui`'s own
   * `handle_question_key` exactly. Returns whether the key was consumed. */
  function answerQuestion(event: KeyboardEvent, job: ActiveJob): boolean {
    const question = job.question;
    if (!question) return false;
    const key = event.key === "Escape" ? "c" : event.key.toLowerCase();

    if (question.kind === "conflict") {
      const actions: Record<string, ConflictAction> = {
        o: "overwrite",
        u: "overwriteOlder",
        s: "skip",
        r: "rename",
        c: "cancel",
      };
      const action = actions[key];
      if (!action) return false;
      void answerConflict(question.data.job, action, event.shiftKey);
    } else {
      const answers: Record<string, ErrorAction> = { r: "retry", s: "skip", a: "skipAll", c: "cancel" };
      const answer = answers[key];
      if (!answer) return false;
      void answerError(question.data.job, answer);
    }
    setActiveJob((j) => (j ? { ...j, question: undefined } : j));
    return true;
  }

  function handleKey(event: KeyboardEvent) {
    if (!props.active()) return;

    const job = activeJob();
    if (job?.question) {
      if (answerQuestion(event, job)) event.preventDefault();
      return;
    }

    const transfer = pendingTransfer();
    if (transfer) {
      if (event.key === "Enter") {
        void runTransfer(transfer, destInput());
        event.preventDefault();
      } else if (event.key === "Escape") {
        setPendingTransfer(null);
        event.preventDefault();
      }
      // Any other key (typing, arrows, Backspace within the field) is left
      // alone so the native <input> handles it itself.
      return;
    }

    const pending = confirmDelete();
    if (pending) {
      if (event.key === "Enter") {
        void runDelete(pending);
      } else if (event.key === "Escape") {
        setConfirmDelete(null);
      } else {
        return;
      }
      event.preventDefault();
      return;
    }

    switch (event.key) {
      case "ArrowDown":
        moveCursor(1);
        break;
      case "ArrowUp":
        moveCursor(-1);
        break;
      case "Enter":
        openCursor();
        break;
      case "Backspace":
        goUp();
        break;
      case "Insert":
      case " ":
        toggleMarkAtCursor();
        break;
      case "Delete":
      case "F8":
        askToDelete(event.shiftKey);
        break;
      case "F5":
        askToTransfer(false);
        break;
      case "F6":
        askToTransfer(true);
        break;
      case "Escape": {
        const job = activeJob();
        if (!job) return;
        void cancelJob(job.id);
        break;
      }
      default:
        return; // let anything else (Tab, for switching panels) bubble up
    }
    event.preventDefault();
  }

  return (
    <div
      ref={container}
      class="panel"
      classList={{ active: props.active() }}
      tabIndex={0}
      onClick={props.onActivate}
      onKeyDown={handleKey}
    >
      <div class="panel-header" title={path()}>
        {path()}
      </div>
      <Show when={pendingTransfer()}>
        {(transfer) => (
          <div class="panel-status confirm transfer">
            <span>
              {transfer().isMove ? "Move" : "Copy"} {transfer().sources.length} item
              {transfer().sources.length === 1 ? "" : "s"} to:
            </span>
            <input
              ref={destInputEl}
              value={destInput()}
              onInput={(e) => setDestInput(e.currentTarget.value)}
            />
          </div>
        )}
      </Show>
      <Show when={confirmDelete()}>
        {(pending) => (
          <div class="panel-status confirm">
            {pending().permanent ? "Delete permanently" : "Delete"} {pending().targets.length}{" "}
            item{pending().targets.length === 1 ? "" : "s"}? Enter to confirm, Esc to cancel.
          </div>
        )}
      </Show>
      <Show when={activeJob()?.question} keyed>
        {(question) =>
          question.kind === "conflict" ? (
            <div class="panel-status question">
              "{question.data.source.name}" already exists (
              {question.data.existing.isDirLike ? "folder" : formatSize(question.data.existing.size)}
              ). O=Overwrite U=if newer S=Skip R=Rename C=Cancel (Shift = for every later conflict)
            </div>
          ) : (
            <div class="panel-status question">
              {question.data.message}. R=Retry S=Skip A=Skip all C=Cancel
            </div>
          )
        }
      </Show>
      <Show when={!activeJob()?.question && activeJob()}>
        {(job) => (
          <div class="panel-status job">
            {job().title}
            {job().progress ? ` — ${Math.round(job().progress!.fraction * 100)}%` : "…"}
            {" (Esc to cancel)"}
          </div>
        )}
      </Show>
      <Show
        when={!listing.loading}
        fallback={<div class="panel-status">Reading...</div>}
      >
        <Show
          when={!listing.error}
          fallback={<div class="panel-status error">{String(listing.error)}</div>}
        >
          <table class="panel-table">
            <thead>
              <tr>
                <th>Name</th>
                <th class="size">Size</th>
                <th class="date">Modified</th>
              </tr>
            </thead>
            <tbody>
              <For each={entries()}>
                {(entry, index) => (
                  <tr
                    classList={{
                      selected: index() === cursor(),
                      marked: marked().has(entry.name),
                    }}
                    onClick={() => {
                      setCursor(index());
                      props.onActivate();
                    }}
                    onDblClick={() => {
                      setCursor(index());
                      openCursor();
                    }}
                  >
                    <td classList={{ name: true, dir: entry.isDirLike }}>{entry.name}</td>
                    <td class="size">{entry.isDirLike ? "" : formatSize(entry.size)}</td>
                    <td class="date">{formatDate(entry.modifiedMs)}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
          <Show when={entries().length === 0}>
            <div class="panel-status">(empty)</div>
          </Show>
        </Show>
      </Show>
    </div>
  );
}
