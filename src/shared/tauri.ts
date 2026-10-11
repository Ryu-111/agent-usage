import type { Agent, AppSnapshot, UsageHistory, UsageWindow } from "./types";

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(
  event: string,
  handler: (event: { payload: T }) => void
) => Promise<() => void>;

export async function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>
): Promise<T> {
  const mod = await import("@tauri-apps/api/core");
  return (mod.invoke as Invoke)<T>(command, args);
}

export async function listenSnapshot(
  handler: (snapshot: AppSnapshot) => void
): Promise<() => void> {
  const mod = await import("@tauri-apps/api/event");
  return (mod.listen as Listen)("usage://snapshot", (event: { payload: AppSnapshot }) =>
    handler(event.payload)
  );
}

export async function loadLiveSnapshot(): Promise<AppSnapshot> {
  const snapshot = await invokeCommand<AppSnapshot | null>("get_usage_snapshot");
  return snapshot ?? invokeCommand<AppSnapshot>("refresh_usage");
}

export async function refreshLiveSnapshot(): Promise<AppSnapshot> {
  return invokeCommand<AppSnapshot>("refresh_usage");
}

export async function loadUsageHistory(): Promise<UsageHistory> {
  return invokeCommand<UsageHistory>("get_usage_history");
}

export function previewHistory(): UsageHistory {
  const hours = 8 * 24;
  const start = new Date();
  start.setUTCMinutes(0, 0, 0);
  start.setTime(start.getTime() - (hours - 1) * 3_600_000);
  // Deterministic, work-hours-shaped sample data for the browser preview.
  const curve = (seed: number) =>
    Array.from({ length: hours }, (_, i) => {
      const hourJst = (start.getUTCHours() + i + 9) % 24;
      const working = hourJst >= 9 && hourJst <= 23 ? 1 : 0.05;
      const day = Math.floor(i / 24);
      return Math.round(working * (6000 + ((day * 7919 + seed * 104729) % 9000)));
    });
  return {
    bucketStart: start.toISOString(),
    bucketMinutes: 60,
    series: [
      { agent: "claudeCode", tokens: curve(1) },
      { agent: "codex", tokens: curve(2).map((value) => Math.round(value * 0.6)) }
    ]
  };
}

export function unavailableSnapshot(): AppSnapshot {
  const agents: Agent[] = ["claudeCode", "codex"];
  const windows: UsageWindow[] = ["fiveHour", "weekly"];

  return {
    capturedAt: new Date().toISOString(),
    agents: agents.map((agent) => ({
      agent,
      windows: windows.map((window) => ({
        agent,
        window,
        utilizationPct: null,
        usedTokens: null,
        burnRateTokensPerMin: null,
        resetAt: null,
        limitReachedAt: null,
        observedAt: null,
        source: "unavailable"
      }))
    }))
  };
}

export function isBrowserPreview(): boolean {
  return import.meta.env.DEV && !("__TAURI_INTERNALS__" in window);
}

export function previewSnapshot(): AppSnapshot {
  return {
    capturedAt: new Date().toISOString(),
    agents: [
      {
        agent: "claudeCode",
        windows: [
          {
            agent: "claudeCode",
            window: "fiveHour",
            utilizationPct: 42,
            usedTokens: 84000,
            burnRateTokensPerMin: 280,
            resetAt: new Date(Date.now() + 92 * 60_000).toISOString(),
            limitReachedAt: null,
            observedAt: new Date().toISOString(),
            source: "localEstimate"
          },
          {
            agent: "claudeCode",
            window: "weekly",
            utilizationPct: 57,
            usedTokens: 620000,
            burnRateTokensPerMin: 61,
            resetAt: new Date(Date.now() + 2.1 * 86_400_000).toISOString(),
            limitReachedAt: null,
            observedAt: new Date().toISOString(),
            source: "official"
          }
        ]
      },
      {
        agent: "codex",
        windows: [
          {
            agent: "codex",
            window: "fiveHour",
            utilizationPct: null,
            usedTokens: 116000,
            burnRateTokensPerMin: 386,
            resetAt: new Date(Date.now() + 174 * 60_000).toISOString(),
            limitReachedAt: null,
            observedAt: new Date().toISOString(),
            source: "localEstimate"
          },
          {
            agent: "codex",
            window: "weekly",
            utilizationPct: null,
            usedTokens: 380000,
            burnRateTokensPerMin: 38,
            resetAt: new Date(Date.now() + 4.7 * 86_400_000).toISOString(),
            limitReachedAt: null,
            observedAt: new Date().toISOString(),
            source: "localEstimate"
          }
        ]
      }
    ]
  };
}
