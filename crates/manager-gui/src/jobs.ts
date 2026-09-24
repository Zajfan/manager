// Thin IPC wrappers around the background job commands — deliberately no
// logic of its own, the same way `fetchListing` in `Panel.tsx` isn't. A job
// keeps reporting back long after `startDelete` returns, so that part is
// Tauri events (`job-progress`, `job-finished`), not a return value.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ConflictQuestionDto,
  ErrorQuestionDto,
  JobProgressEvent,
  JobReportDto,
  JobStartedDto,
} from "./types";

export function startDelete(targets: string[], permanent: boolean): Promise<JobStartedDto> {
  return invoke<JobStartedDto>("start_delete", { targets, permanent });
}

export function startTransfer(
  sources: string[],
  base: string,
  dest: string,
  isMove: boolean,
): Promise<JobStartedDto> {
  return invoke<JobStartedDto>("start_transfer", { sources, base, dest, isMove });
}

export function cancelJob(id: number): Promise<void> {
  return invoke("job_action", { id, action: "cancel" });
}

export function onJobProgress(handler: (event: JobProgressEvent) => void): Promise<UnlistenFn> {
  return listen<JobProgressEvent>("job-progress", (e) => handler(e.payload));
}

export function onJobFinished(handler: (report: JobReportDto) => void): Promise<UnlistenFn> {
  return listen<JobReportDto>("job-finished", (e) => handler(e.payload));
}

export function onJobConflict(
  handler: (question: ConflictQuestionDto) => void,
): Promise<UnlistenFn> {
  return listen<ConflictQuestionDto>("job-conflict", (e) => handler(e.payload));
}

export function onJobError(handler: (question: ErrorQuestionDto) => void): Promise<UnlistenFn> {
  return listen<ErrorQuestionDto>("job-error", (e) => handler(e.payload));
}

export type ConflictAction = "overwrite" | "overwriteOlder" | "skip" | "rename" | "cancel";

export function answerConflict(
  job: number,
  action: ConflictAction,
  applyToAll: boolean,
): Promise<void> {
  return invoke("answer_conflict", { job, action, applyToAll });
}

export type ErrorAction = "retry" | "skip" | "skipAll" | "cancel";

export function answerError(job: number, answer: ErrorAction): Promise<void> {
  return invoke("answer_error", { job, answer });
}
