// One pane: a folder's contents, its own cursor, and the keys that move
// around and open things. Like `manager-tui`'s `Panel`, this holds no
// opinion about sorting or filtering — it just shows what `list_dir` (Rust,
// the same code the terminal version calls) already decided.

import { For, Show, createEffect, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { cancelJob, onJobFinished, onJobProgress, startDelete } from "./jobs";
import {
  clampCursor,
  entryToOpen,
  moveCursor as moveCursorTo,
  selection,
  toggleMark,
} from "./panelLogic";
import type { EntryDto, ListingDto, ProgressDto } from "./types";
import { formatDate, formatSize } from "./format";

interface ActiveJob {
  id: number;
  title: string;
  progress?: ProgressDto;
}

interface PendingDelete {
  targets: EntryDto[];
  permanent: boolean;
}

export interface PanelProps {
  initialPath: string;
  active: () => boolean;
  onActivate: () => void;
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
  const [activeJob, setActiveJob] = createSignal<ActiveJob | null>(null);
  const [listing, { refetch }] = createResource(path, fetchListing);

  onMount(() => {
    const progress = onJobProgress((event) => {
      setActiveJob((job) => (job && job.id === event.id ? { ...job, progress: event.progress } : job));
    });
    const finished = onJobFinished((report) => {
      if (activeJob()?.id !== report.id) return;
      setActiveJob(null);
      refetch();
    });
    onCleanup(() => {
      void progress.then((unlisten) => unlisten());
      void finished.then((unlisten) => unlisten());
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

  function handleKey(event: KeyboardEvent) {
    if (!props.active()) return;

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
      <Show when={confirmDelete()}>
        {(pending) => (
          <div class="panel-status confirm">
            {pending().permanent ? "Delete permanently" : "Delete"} {pending().targets.length}{" "}
            item{pending().targets.length === 1 ? "" : "s"}? Enter to confirm, Esc to cancel.
          </div>
        )}
      </Show>
      <Show when={activeJob()}>
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
