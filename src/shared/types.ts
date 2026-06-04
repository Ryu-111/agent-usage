export type Agent = "claudeCode" | "codex";
export type UsageWindow = "fiveHour" | "weekly";
export type SnapshotSource = "official" | "localEstimate" | "unavailable";

export interface UsageSnapshot {
  agent: Agent;
  window: UsageWindow;
  utilizationPct: number | null;
  usedTokens: number | null;
  burnRateTokensPerMin: number | null;
  resetAt: string | null;
  limitReachedAt: string | null;
  source: SnapshotSource;
}

export interface AgentUsage {
  agent: Agent;
  windows: UsageSnapshot[];
}

export interface AppSnapshot {
  capturedAt: string;
  agents: AgentUsage[];
}

export const agentLabel: Record<Agent, string> = {
  claudeCode: "Claude Code",
  codex: "Codex"
};

export const windowLabel: Record<UsageWindow, string> = {
  fiveHour: "5h",
  weekly: "Week"
};
