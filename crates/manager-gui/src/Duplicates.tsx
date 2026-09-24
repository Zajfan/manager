// Ctrl+D's dialog and its results, as one overlay — the same two-phase
// shape Search.tsx and Compare.tsx have. Groups arrive live while the
// search is still running, matching `manager-tui`'s own `Duplicates` view.
//
// Deleting what's marked is an ordinary startDelete call, same as F8 — one
// job, no possible conflict (nothing here is a destination). Its progress
// and any error still have to be answered before this overlay can close,
// the same reasoning Compare.tsx's sync jobs follow.

import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { answerError, onJobError, onJobFinished, onJobProgress, startDelete, type ErrorAction } from "./jobs";
import {
  cancelDuplicates,
  onDuplicatesFinished,
  onDuplicatesFound,
  onDuplicatesProgress,
  startDuplicates,
} from "./duplicatesApi";
import { flattenGroups, markAllButFirstOfEachGroup, selection, type Row } from "./duplicatesLogic";
import { clampCursor, moveCursor as moveCursorTo, toggleMark } from "./panelLogic";
import type { DuplicateGroupDto, DuplicateReportDto, ErrorQuestionDto, ProgressDto } from "./types";
import { formatSize } from "./format";

export interface DuplicatesProps {
  rootPath: string;
  onClose: () => void;
}

interface ActiveJob {
  id: number;
  title: string;
  progress?: ProgressDto;
  question?: ErrorQuestionDto;
}

export function Duplicates(props: DuplicatesProps) {
  const [phase, setPhase] = createSignal<"dialog" | "results">("dialog");
  const [mask, setMask] = createSignal("*");
  const [includeHidden, setIncludeHidden] = createSignal(false);

  const [groups, setGroups] = createSignal<DuplicateGroupDto[]>([]);
  const [cursor, setCursor] = createSignal(0);
  const [marked, setMarked] = createSignal<Set<string>>(new Set());
  const [scanned, setScanned] = createSignal(0);
  const [running, setRunning] = createSignal(true);
  const [report, setReport] = createSignal<DuplicateReportDto | null>(null);
  const [confirmDelete, setConfirmDelete] = createSignal(false);
  const [activeJob, setActiveJob] = createSignal<ActiveJob | null>(null);

  const rows = (): Row[] => flattenGroups(groups());

  let container: HTMLDivElement | undefined;
  let maskInput: HTMLInputElement | undefined;
  onMount(() => {
    container?.focus();
    maskInput?.focus();
    maskInput?.select();
  });

  onMount(() => {
    const found = onDuplicatesFound((group) => setGroups((gs) => [...gs, group]));
    const progress = onDuplicatesProgress((p) => setScanned(p.scanned));
    const finished = onDuplicatesFinished((r) => {
      setRunning(false);
      setReport(r);
    });
    const jobProgress = onJobProgress((event) => {
      setActiveJob((job) => (job && job.id === event.id ? { ...job, progress: event.progress } : job));
    });
    const jobFinished = onJobFinished((r) => {
      if (activeJob()?.id !== r.id) return;
      setActiveJob(null);
    });
    const jobError = onJobError((data) => {
      setActiveJob((job) => (job && job.id === data.job ? { ...job, question: data } : job));
    });
    onCleanup(() => {
      void found.then((unlisten) => unlisten());
      void progress.then((unlisten) => unlisten());
      void finished.then((unlisten) => unlisten());
      void jobProgress.then((unlisten) => unlisten());
      void jobFinished.then((unlisten) => unlisten());
      void jobError.then((unlisten) => unlisten());
      void cancelDuplicates();
    });
  });

  createEffect(() => {
    setCursor((c) => clampCursor(c, rows().length));
  });

  function submit() {
    setGroups([]);
    setCursor(0);
    setMarked(new Set<string>());
    setScanned(0);
    setRunning(true);
    setReport(null);
    setPhase("results");
    void startDuplicates(props.rootPath, mask().trim() || "*", includeHidden());
  }

  function moveCursor(delta: number) {
    setCursor((c) => moveCursorTo(c, delta, rows().length));
  }

  function toggleMarkAtCursor() {
    const row = rows()[cursor()];
    if (row?.kind === "file") setMarked((m) => toggleMark(m, row.entry.path));
  }

  async function runDelete() {
    const targets = selection(rows(), marked(), cursor());
    setConfirmDelete(false);
    if (targets.length === 0) return;
    setMarked(new Set<string>());
    const started = await startDelete(
      targets.map((e) => e.path),
      false,
    );
    setActiveJob({ id: started.id, title: started.title });
  }

  function answerQuestion(event: KeyboardEvent, job: ActiveJob): boolean {
    if (!job.question) return false;
    const answers: Record<string, ErrorAction> = { r: "retry", s: "skip", a: "skipAll", c: "cancel" };
    const key = event.key === "Escape" ? "c" : event.key.toLowerCase();
    const answer = answers[key];
    if (!answer) return false;
    void answerError(job.question.job, answer);
    setActiveJob((j) => (j ? { ...j, question: undefined } : j));
    return true;
  }

  function handleKey(event: KeyboardEvent) {
    event.stopPropagation();

    if (phase() === "dialog") {
      if (event.key === "Enter") {
        submit();
      } else if (event.key === "Escape") {
        props.onClose();
      } else if (event.altKey && event.key.toLowerCase() === "h") {
        setIncludeHidden((h) => !h);
      } else {
        return;
      }
      event.preventDefault();
      return;
    }

    const job = activeJob();
    if (job?.question) {
      if (answerQuestion(event, job)) event.preventDefault();
      return;
    }

    if (confirmDelete()) {
      if (event.key === "Enter") {
        void runDelete();
      } else if (event.key === "Escape") {
        setConfirmDelete(false);
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
      case "Insert":
      case " ":
        toggleMarkAtCursor();
        break;
      case "k":
      case "K":
        setMarked(markAllButFirstOfEachGroup(groups()));
        break;
      case "d":
      case "D":
      case "Delete":
        if (selection(rows(), marked(), cursor()).length > 0) setConfirmDelete(true);
        break;
      case "Escape":
      case "F10":
        // Closing while the delete job is still running would tear down
        // the only listener that can answer a later error from it.
        if (!activeJob()) props.onClose();
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  return (
    <div ref={container} class="overlay" tabIndex={0} onKeyDown={handleKey}>
      <Show when={phase() === "dialog"}>
        <div class="overlay-title">Find duplicate files</div>
        <label class="overlay-field">
          Named
          <input ref={maskInput} value={mask()} onInput={(e) => setMask(e.currentTarget.value)} />
        </label>
        <label class="overlay-field checkbox">
          <input
            type="checkbox"
            checked={includeHidden()}
            onChange={(e) => setIncludeHidden(e.currentTarget.checked)}
          />
          Include hidden (Alt+H)
        </label>
        <div class="overlay-hint">Enter to search, Esc to cancel</div>
      </Show>

      <Show when={phase() === "results"}>
        <div class="overlay-title">{mask()} in {props.rootPath}</div>
        <div class="overlay-hint">
          {running()
            ? `Looking for duplicates… ${scanned()} scanned, ${groups().length} groups found`
            : report()?.groups === 0
              ? "No duplicates found"
              : `${groups().length} duplicate group${groups().length === 1 ? "" : "s"} — ${formatSize(
                  groups().reduce((sum, g) => sum + g.size * (g.files.length - 1), 0),
                )} could be freed` +
                (report()?.cancelled ? " (stopped)" : "")}
        </div>
        <table class="panel-table">
          <tbody>
            <For each={rows()}>
              {(row, index) =>
                row.kind === "groupStart" ? (
                  <tr class="group-start" classList={{ selected: index() === cursor() }}>
                    <td colSpan={2}>{formatSize(row.size)} each</td>
                  </tr>
                ) : (
                  <tr
                    classList={{
                      selected: index() === cursor(),
                      marked: marked().has(row.entry.path),
                    }}
                    onClick={() => setCursor(index())}
                  >
                    <td class="name">{row.entry.path}</td>
                    <td class="size">{formatSize(row.entry.size)}</td>
                  </tr>
                )
              }
            </For>
          </tbody>
        </table>
        <Show when={confirmDelete()}>
          <div class="panel-status confirm">
            Move {selection(rows(), marked(), cursor()).length} item
            {selection(rows(), marked(), cursor()).length === 1 ? "" : "s"} to the trash? Enter to
            confirm, Esc to cancel.
          </div>
        </Show>
        <Show when={activeJob()}>
          {(job) => (
            <Show
              when={job().question}
              fallback={
                <div class="panel-status job">
                  {job().title}
                  {job().progress ? ` — ${Math.round(job().progress!.fraction * 100)}%` : "…"}
                </div>
              }
            >
              {(question) => (
                <div class="panel-status question">
                  {question().message}. R=Retry S=Skip A=Skip all C=Cancel
                </div>
              )}
            </Show>
          )}
        </Show>
        <div class="overlay-hint">
          Space marks, K keeps the first of each group and marks the rest, D deletes the marked
          (or the one under the cursor), Esc closes
        </div>
      </Show>
    </div>
  );
}
