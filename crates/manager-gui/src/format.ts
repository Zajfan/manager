// Small, dependency-free formatting helpers — the same rules the terminal
// version's `format.rs` uses, so a folder looks the same size and age in
// both.

export function formatSize(bytes: number): string {
  const units = ["B", "K", "M", "G", "T"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${bytes} B` : `${value.toFixed(1)}${units[unit]}`;
}

export function formatDate(modifiedMs: number | null): string {
  if (modifiedMs === null) return "";
  const date = new Date(modifiedMs);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
