// One pane: a folder's contents, its own cursor, and the keys that move
// around and open things. Like `manager-tui`'s `Panel`, this holds no
// opinion about sorting or filtering — it just shows what `list_dir` (Rust,
// the same code the terminal version calls) already decided.

import { For, Show, createEffect, createResource, createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { clampCursor, entryToOpen, moveCursor as moveCursorTo } from "./panelLogic";
import type { EntryDto, ListingDto } from "./types";
import { formatDate, formatSize } from "./format";

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
  const [listing] = createResource(path, fetchListing);

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
    }
  }

  function goUp() {
    const parent = listing()?.parent;
    if (parent) {
      setPath(parent);
      setCursor(0);
    }
  }

  function handleKey(event: KeyboardEvent) {
    if (!props.active()) return;
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
                    classList={{ selected: index() === cursor() }}
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
