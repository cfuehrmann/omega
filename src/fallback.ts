export function shouldFallbackToCodex(err: any, fallbackEnabled: boolean): boolean {
  if (!fallbackEnabled) return false;
  if (!err) return false;
  const status = err.status ?? err.statusCode;
  if (status === 429) return true;
  const msg = typeof err.message === "string" ? err.message.toLowerCase() : "";
  if (msg.includes("rate limit")) return true;
  return false;
}
