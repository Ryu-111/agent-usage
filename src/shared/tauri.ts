import type { Agent, AppSnapshot, UsageWindow } from "./types";

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
