// Alt+F7's dialog and its results, as one overlay: a small form (what to
// look for, and where) that turns into a live-updating list once submitted.
// Like `manager-tui`'s `Results`, hits arrive while the search is still
// running — the list has to be usable before it's complete.

import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { cancelSearch, onSearchFinished, onSearchFound, onSearchProgress, startSearch } from "./searchApi";
import { moveCursor as moveCursorTo } from "./panelLogic";
import type { EntryDto, SearchReportDto } from "./types";
import { formatSize } from "./format";

export interface SearchProps {
  /** The folder to search under — the active panel's path at the moment
   * Alt+F7 was pressed. */
  rootPath: string;
  onClose: () => void;
  /** Takes the active panel to this file and closes the overlay. */
  onGoto: (path: string) => void;
}

export function Search(props: SearchProps) {
  const [phase, setPhase] = createSignal<"dialog" | "results">("dialog");
  const [mask, setMask] = createSignal("*");
  const [text, setText] = createSignal("");
  const [caseSensitive, setCaseSensitive] = createSignal(false);
  const [includeHidden, setIncludeHidden] = createSignal(false);

  const [summary, setSummary] = createSignal("");
  const [hits, setHits] = createSignal<EntryDto[]>([]);
  const [cursor, setCursor] = createSignal(0);
  const [scanned, setScanned] = createSignal(0);
  const [running, setRunning] = createSignal(true);
  const [report, setReport] = createSignal<SearchReportDto | null>(null);

  let container: HTMLDivElement | undefined;
  let maskInput: HTMLInputElement | undefined;
  onMount(() => {
    container?.focus();
    maskInput?.focus();
    maskInput?.select();
  });

  onMount(() => {
    const found = onSearchFound((entry) => setHits((h) => [...h, entry]));
    const progress = onSearchProgress((p) => setScanned(p.scanned));
    const finished = onSearchFinished((r) => {
      setRunning(false);
      setReport(r);
    });
    onCleanup(() => {
      void found.then((unlisten) => unlisten());
      void progress.then((unlisten) => unlisten());
      void finished.then((unlisten) => unlisten());
      // Leaving the results (or never reaching them) shouldn't leave a
      // search running unseen in the background.
      void cancelSearch();
    });
  });

  function submit() {
    const m = mask().trim() || "*";
    const t = text().trim();
    setSummary(t ? `${m} containing "${t}" in ${props.rootPath}` : `${m} in ${props.rootPath}`);
    setHits([]);
    setCursor(0);
    setScanned(0);
    setRunning(true);
    setReport(null);
    setPhase("results");
    void startSearch(props.rootPath, m, t, caseSensitive(), includeHidden());
  }

  function moveCursor(delta: number) {
    setCursor((c) => moveCursorTo(c, delta, hits().length));
  }

  function goto() {
    const entry = hits()[cursor()];
    if (entry) props.onGoto(entry.path);
  }

  // Stop propagation up to the app — Tab, arrows and letters here must never
  // fall through to panel navigation or the panel-switch shortcut.
  function handleKey(event: KeyboardEvent) {
    event.stopPropagation();

    if (phase() === "dialog") {
      if (event.key === "Enter") {
        submit();
      } else if (event.key === "Escape") {
        props.onClose();
      } else if (event.altKey && event.key.toLowerCase() === "c") {
        setCaseSensitive((c) => !c);
      } else if (event.altKey && event.key.toLowerCase() === "h") {
        setIncludeHidden((h) => !h);
      } else {
        return; // let the focused <input> handle its own typing
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
        goto();
        break;
      case "Escape":
        props.onClose();
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  return (
    <div ref={container} class="overlay" tabIndex={0} onKeyDown={handleKey}>
      <Show when={phase() === "dialog"}>
        <div class="overlay-title">Find files</div>
        <label class="overlay-field">
          Named
          <input
            ref={maskInput}
            value={mask()}
            onInput={(e) => setMask(e.currentTarget.value)}
          />
        </label>
        <label class="overlay-field">
          Containing
          <input value={text()} onInput={(e) => setText(e.currentTarget.value)} />
        </label>
        <label class="overlay-field checkbox">
          <input
            type="checkbox"
            checked={caseSensitive()}
            onChange={(e) => setCaseSensitive(e.currentTarget.checked)}
          />
          Match case (Alt+C)
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
        <div class="overlay-title">{summary()}</div>
        <div class="overlay-hint">
          {running()
            ? `Searching… ${scanned()} scanned, ${hits().length} found`
            : `Done — ${hits().length} found, ${report()?.scanned ?? scanned()} scanned` +
              (report()?.unreadable ? `, ${report()!.unreadable} unreadable` : "") +
              (report()?.cancelled ? " (stopped)" : "")}
        </div>
        <table class="panel-table">
          <tbody>
            <For each={hits()}>
              {(entry, index) => (
                <tr classList={{ selected: index() === cursor() }} onClick={() => setCursor(index())}>
                  <td class="name">{entry.path}</td>
                  <td class="size">{entry.isDirLike ? "" : formatSize(entry.size)}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
        <Show when={hits().length === 0 && !running()}>
          <div class="panel-status">(nothing found)</div>
        </Show>
        <div class="overlay-hint">Enter to go there, Esc to close</div>
      </Show>
    </div>
  );
}
