//! Typed IPC: unwrap generated status unions, throwing human errors.
//! Call sites `await call(commands.foo())` — never silent, never untyped.

type StatusResult<T> =
  | { status: "ok"; data: T }
  | { status: "error"; error: unknown };

export async function call<T>(p: Promise<StatusResult<T>>): Promise<T> {
  const r = await p;
  if (r.status === "error") {
    throw new Error(typeof r.error === "string" ? r.error : JSON.stringify(r.error));
  }
  return r.data;
}

export function fmtGB(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

export function fmtMB(mb: number): string {
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${mb.toFixed(1)} MB`;
}
