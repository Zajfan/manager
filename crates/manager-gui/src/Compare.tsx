// Ctrl+F9's dialog and its results, as one overlay — the same two-phase
// shape Search.tsx has. Differences arrive live while the comparison is
// still running, matching `manager-tui`'s own `Compare` view.
//
// Syncing a difference starts a job through the ordinary job engine
// (`syncCompare` → the same `manager_core::jobs` a plain F5 uses), so its
// progress, conflicts and errors arrive as the same job-progress/
// job-conflict/job-error events Panel.tsx already answers — this view just
// answers them too, for whichever jobs it started. Unlike the terminal
// version (which hands sync jobs to its always-on global job system and
// closes immediately) this overlay has to stay open and keep listening
// until every job it started is done — closing early would mean a
// conflict or error question nobody is left to answer.

import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import {
  answerConflict,
  answerError,
  onJobConflict,
  onJobError,
  onJobFinished,
  onJobProgress,
  type ConflictAction,
  type ErrorAction,
} from "./jobs";
import {
  cancelCompare,
  onCompareFinished,
  onCompareFound,
  onCompareProgress,
  startCompare,
  syncCompare,
} from "./compareApi";
import { isSyncable, markAllSyncable, selection } from "./compareLogic";
import { clampCursor, moveCursor as moveCursorTo, toggleMark } from "./panelLogic";
import type {
  CompareReportDto,
  ConflictQuestionDto,
  DiffEntryDto,
  EntryDto,
  ErrorQuestionDto,
  ProgressDto,
  SyncDirection,
} from "./types";
import { formatDate, formatSize } from "./format";

export interface CompareProps {
  left: string;
  right: string;
  onClose: () => void;
}

interface SyncJob {
  id: number;
  title: string;
  progress?: ProgressDto;
  question?:
    | { kind: "conflict"; data: ConflictQuestionDto }
    | { kind: "error"; data: ErrorQuestionDto };
}

export function Compare(props: CompareProps) {
  const [phase, setPhase] = createSignal<"dialog" | "results">("dialog");
  const [byContent, setByContent] = createSignal(false);
  const [includeHidden, setIncludeHidden] = createSignal(false);

  const [entries, setEntries] = createSignal<DiffEntryDto[]>([]);
  const [cursor, setCursor] = createSignal(0);
  const [marked, setMarked] = createSignal<Set<string>>(new Set());
  const [scanned, setScanned] = createSignal(0);
  const [running, setRunning] = createSignal(true);
  const [report, setReport] = createSignal<CompareReportDto | null>(null);
  const [syncJobs, setSyncJobs] = createSignal<SyncJob[]>([]);

  let container: HTMLDivElement | undefined;
  onMount(() => container?.focus());

  onMount(() => {
    const found = onCompareFound((entry) => setEntries((es) => [...es, entry]));
    const progress = onCompareProgress((p) => setScanned(p.scanned));
    const finished = onCompareFinished((r) => {
      setRunning(false);
      setReport(r);
    });
    const jobProgress = onJobProgress((event) => {
      setSyncJobs((jobs) =>
        jobs.map((j) => (j.id === event.id ? { ...j, progress: event.progress } : j)),
      );
    });
    const jobFinished = onJobFinished((r) => {
      setSyncJobs((jobs) => jobs.filter((j) => j.id !== r.id));
    });
    const jobConflict = onJobConflict((data) => {
      setSyncJobs((jobs) =>
        jobs.map((j) => (j.id === data.job ? { ...j, question: { kind: "conflict", data } } : j)),
      );
    });
    const jobError = onJobError((data) => {
      setSyncJobs((jobs) =>
        jobs.map((j) => (j.id === data.job ? { ...j, question: { kind: "error", data } } : j)),
      );
    });
    onCleanup(() => {
      void found.then((unlisten) => unlisten());
      void progress.then((unlisten) => unlisten());
      void finished.then((unlisten) => unlisten());
      void jobProgress.then((unlisten) => unlisten());
      void jobFinished.then((unlisten) => unlisten());
      void jobConflict.then((unlisten) => unlisten());
      void jobError.then((unlisten) => unlisten());
      void cancelCompare();
    });
  });

  createEffect(() => {
    setCursor((c) => clampCursor(c, entries().length));
  });

  function submit() {
    setEntries([]);
    setCursor(0);
    setMarked(new Set<string>());
    setScanned(0);
    setRunning(true);
    setReport(null);
    setPhase("results");
    void startCompare(props.left, props.right, byContent(), includeHidden());
  }

  function moveCursor(delta: number) {
    setCursor((c) => moveCursorTo(c, delta, entries().length));
  }

  function toggleMarkAtCursor() {
    const entry = entries()[cursor()];
    if (entry && isSyncable(entry.status)) setMarked((m) => toggleMark(m, entry.key));
  }

  async function sync(direction: SyncDirection) {
    const targets = selection(entries(), marked(), cursor());
    if (targets.length === 0) return;
    const started = await syncCompare(
      targets.map((e) => e.key),
      direction,
    );
    setSyncJobs((jobs) => [...jobs, ...started.map((s) => ({ id: s.id, title: s.title }))]);
    setMarked(new Set<string>());
  }

  /** Same key mapping and shape as Panel's own `answerQuestion`. */
  function answerQuestion(event: KeyboardEvent, job: SyncJob): boolean {
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
    setSyncJobs((jobs) => jobs.map((j) => (j.id === job.id ? { ...j, question: undefined } : j)));
    return true;
  }

  function handleKey(event: KeyboardEvent) {
    event.stopPropagation();

    if (phase() === "dialog") {
      if (event.key === "Enter") {
        submit();
      } else if (event.key === "Escape") {
        props.onClose();
      } else if (event.altKey && event.key.toLowerCase() === "c") {
        setByContent((c) => !c);
      } else if (event.altKey && event.key.toLowerCase() === "h") {
        setIncludeHidden((h) => !h);
      } else {
        return;
      }
      event.preventDefault();
      return;
    }

    const withQuestion = syncJobs().find((j) => j.question);
    if (withQuestion) {
      if (answerQuestion(event, withQuestion)) event.preventDefault();
      return;
    }

    switch (event.key) {
      case "ArrowDown":
        moveCursor(1);
        break;
      case "ArrowUp":
        moveCursor(-1);
        break;
      case "Insert":
      case " ":
        toggleMarkAtCursor();
        break;
      case "a":
      case "A":
        setMarked(markAllSyncable(entries()));
        break;
      case ">":
        void sync("leftToRight");
        break;
      case "<":
        void sync("rightToLeft");
        break;
      case "u":
      case "U":
        void sync("newer");
        break;
      case "Escape":
      case "F10":
        // Closing while a sync job is still running would tear down the
        // only listener that can answer its next conflict or error.
        if (syncJobs().length === 0) props.onClose();
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  function cell(entry: EntryDto | null): string {
    return entry ? formatSize(entry.size) + " " + formatDate(entry.modifiedMs) : "—";
  }

  return (
    <div ref={container} class="overlay" tabIndex={0} onKeyDown={handleKey}>
      <Show when={phase() === "dialog"}>
        <div class="overlay-title">Compare folders</div>
        <label class="overlay-field checkbox">
          <input
            type="checkbox"
            checked={byContent()}
            onChange={(e) => setByContent(e.currentTarget.checked)}
          />
          Check content, not just size and date (Alt+C)
        </label>
        <label class="overlay-field checkbox">
          <input
            type="checkbox"
            checked={includeHidden()}
            onChange={(e) => setIncludeHidden(e.currentTarget.checked)}
          />
          Include hidden (Alt+H)
        </label>
        <div class="overlay-hint">Enter to compare, Esc to cancel</div>
      </Show>

      <Show when={phase() === "results"}>
        <div class="overlay-title">
          {props.left} vs {props.right}
        </div>
        <div class="overlay-hint">
          {running()
            ? `Comparing… ${scanned()} scanned, ${entries().length} found`
            : report()?.differences === 0
              ? "No differences"
              : `${entries().length} difference${entries().length === 1 ? "" : "s"}` +
                (report()?.unreadable ? `, ${report()!.unreadable} unreadable` : "") +
                (report()?.cancelled ? " (stopped)" : "")}
        </div>
        <table class="panel-table compare-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Left</th>
              <th>Right</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            <For each={entries()}>
              {(entry, index) => (
                <tr
                  classList={{
                    selected: index() === cursor(),
                    marked: marked().has(entry.key),
                  }}
                  onClick={() => setCursor(index())}
                >
                  <td class="name">{entry.name}</td>
                  <td class="size">{cell(entry.left)}</td>
                  <td class="size">{cell(entry.right)}</td>
                  <td class={`status ${entry.status}`}>{entry.status}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
        <div class="overlay-hint">
          Space marks (only what's syncable), A marks all, &gt; copies left→right, &lt;
          right→left, U copies whichever's newer, Esc closes
        </div>
        <For each={syncJobs()}>
          {(job) => (
            <Show
              when={job.question}
              fallback={
                <div class="panel-status job">
                  {job.title}
                  {job.progress ? ` — ${Math.round(job.progress.fraction * 100)}%` : "…"}
                </div>
              }
            >
              {(question) =>
                question().kind === "conflict" ? (
                  <div class="panel-status question">
                    "{(question().data as ConflictQuestionDto).source.name}" already exists.
                    O=Overwrite U=if newer S=Skip R=Rename C=Cancel (Shift = for every later
                    conflict)
                  </div>
                ) : (
                  <div class="panel-status question">
                    {(question().data as ErrorQuestionDto).message}. R=Retry S=Skip A=Skip all
                    C=Cancel
                  </div>
                )
              }
            </Show>
          )}
        </For>
      </Show>
    </div>
  );
}
