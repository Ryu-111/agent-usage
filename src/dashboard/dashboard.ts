import "../shared/base.css";
import "./dashboard.css";
import { agentLabel, windowLabel, type AppSnapshot, type UsageSnapshot } from "../shared/types";
import { demoSnapshot, invokeCommand, listenSnapshot } from "../shared/tauri";

const rings = document.querySelector<HTMLDivElement>("#rings");
const burnRates = document.querySelector<HTMLDivElement>("#burn-rates");
const refresh = document.querySelector<HTMLButtonElement>("#refresh");

function percent(snapshot: UsageSnapshot): number {
  if (typeof snapshot.utilizationPct === "number") {
    return Math.max(0, Math.min(100, snapshot.utilizationPct));
  }
  if (!snapshot.usedTokens) {
    return 0;
  }
  const softCeiling = snapshot.window === "fiveHour" ? 250_000 : 1_250_000;
  return Math.max(0, Math.min(100, (snapshot.usedTokens / softCeiling) * 100));
}

function formatTokens(value: number | null): string {
  if (!value) {
    return "No data";
  }
  return new Intl.NumberFormat(undefined, { notation: "compact" }).format(value);
}

function formatReset(value: string | null): string {
  if (!value) {
    return "Reset unknown";
  }
  const minutes = Math.max(0, Math.round((new Date(value).getTime() - Date.now()) / 60_000));
  if (minutes >= 1440) {
    return `${Math.round(minutes / 1440)}d to reset`;
  }
  if (minutes >= 60) {
    return `${Math.round(minutes / 60)}h to reset`;
  }
  return `${minutes}m to reset`;
}

function render(snapshot: AppSnapshot): void {
  if (!rings || !burnRates) {
    return;
  }

  const windows = snapshot.agents.flatMap((agent) => agent.windows);
  rings.replaceChildren(
    ...windows.map((usage) => {
      const card = document.createElement("article");
      card.className = "ring-card";
      const pct = percent(usage);
      card.innerHTML = `
        <div class="ring" style="--pct: ${pct}">
          <span>${Math.round(pct)}%</span>
        </div>
        <div>
          <h2>${agentLabel[usage.agent]}</h2>
          <p>${windowLabel[usage.window]} · ${usage.source}</p>
          <strong>${formatTokens(usage.usedTokens)}</strong>
          <small>${formatReset(usage.resetAt)}</small>
        </div>
      `;
      return card;
    })
  );

  burnRates.replaceChildren(
    ...windows.map((usage) => {
      const row = document.createElement("div");
      row.className = "metric-row";
      row.innerHTML = `
        <span>${agentLabel[usage.agent]} ${windowLabel[usage.window]}</span>
        <strong>${Math.round(usage.burnRateTokensPerMin ?? 0).toLocaleString()} tok/min</strong>
      `;
      return row;
    })
  );
}

async function load(): Promise<void> {
  try {
    const snapshot = await invokeCommand<AppSnapshot | null>("get_usage_snapshot");
    render(snapshot ?? demoSnapshot());
  } catch {
    render(demoSnapshot());
  }
}

refresh?.addEventListener("click", async () => {
  try {
    render(await invokeCommand<AppSnapshot>("refresh_usage"));
  } catch {
    await load();
  }
});

void listenSnapshot(render).catch(() => undefined);
void load();
