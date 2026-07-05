import "../shared/base.css";
import "./dashboard.css";
import {
  agentLabel,
  sourceLabel,
  windowLabel,
  type AppSnapshot,
  type UsageSnapshot
} from "../shared/types";
import {
  invokeCommand,
  isBrowserPreview,
  listenSnapshot,
  loadLiveSnapshot,
  previewSnapshot,
  refreshLiveSnapshot,
  unavailableSnapshot
} from "../shared/tauri";

const rings = document.querySelector<HTMLDivElement>("#rings");
const burnRates = document.querySelector<HTMLDivElement>("#burn-rates");
const refresh = document.querySelector<HTMLButtonElement>("#refresh");
const status = document.querySelector<HTMLParagraphElement>("#status");
const hookSettings = document.querySelector<HTMLElement>("#hook-settings");

interface ClaudeHookStatus {
  installed: boolean;
  cacheExists: boolean;
  cacheFresh: boolean;
  cachePath: string | null;
  desktopTokensExists: boolean;
  desktopTokensFresh: boolean;
  desktopTokensPath: string | null;
  desktopBridgeEnabled: boolean;
}

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

function formatTokens(snapshot: UsageSnapshot): string {
  if (snapshot.source === "unavailable" || snapshot.usedTokens === null) {
    return "No data";
  }
  return new Intl.NumberFormat(undefined, { notation: "compact" }).format(snapshot.usedTokens);
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

function formatAge(value: string | null): string {
  if (!value) {
    return "";
  }
  const minutes = Math.max(0, Math.round((Date.now() - new Date(value).getTime()) / 60_000));
  if (minutes <= 2) {
    return "";
  }
  if (minutes >= 1440) {
    return ` · ${Math.round(minutes / 1440)}d old`;
  }
  if (minutes >= 60) {
    return ` · ${Math.round(minutes / 60)}h old`;
  }
  return ` · ${minutes}m old`;
}

function setStatus(message: string): void {
  if (status) {
    status.textContent = message;
  }
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
      card.dataset.source = usage.source;
      const pct = percent(usage);
      card.innerHTML = `
        <div class="ring" style="--pct: ${pct}">
          <span>${Math.round(pct)}%</span>
        </div>
        <div>
          <h2>${agentLabel[usage.agent]}</h2>
          <p>${windowLabel[usage.window]} · ${sourceLabel[usage.source]}${formatAge(usage.observedAt)}</p>
          <strong>${formatTokens(usage)}</strong>
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
      const burnRate =
        usage.source === "unavailable"
          ? "No data"
          : `${Math.round(usage.burnRateTokensPerMin ?? 0).toLocaleString()} tok/min`;
      row.innerHTML = `
        <span>${agentLabel[usage.agent]} ${windowLabel[usage.window]}</span>
        <strong>${burnRate}</strong>
      `;
      return row;
    })
  );
}

async function load(): Promise<void> {
  setStatus("Loading live usage...");
  try {
    render(await loadLiveSnapshot());
    setStatus("Live usage connected");
  } catch {
    render(isBrowserPreview() ? previewSnapshot() : unavailableSnapshot());
    setStatus(isBrowserPreview() ? "Browser preview data" : "Live usage unavailable");
  }
}

async function loadHookSettings(): Promise<void> {
  if (!hookSettings || isBrowserPreview()) {
    hookSettings?.replaceChildren();
    return;
  }
  try {
    const hookStatus = await invokeCommand<ClaudeHookStatus>("get_claude_rate_limits_hook_status");
    hookSettings.replaceChildren();
    if (hookStatus.cacheFresh) {
      hookSettings.textContent = "Claude Code hook connected";
      return;
    }
    if (hookStatus.desktopTokensFresh) {
      hookSettings.textContent = "Claude Desktop token cache connected";
      return;
    }
    if (hookStatus.installed) {
      const text = document.createElement("span");
      if (hookStatus.cacheExists) {
        text.textContent = "Claude Code hook waiting for fresh rate-limit data";
      } else if (hookStatus.desktopTokensExists) {
        text.textContent = "Claude Desktop token cache is present but stale";
      } else if (hookStatus.desktopBridgeEnabled) {
        text.textContent = "Claude Desktop bridge detected. Waiting for token cache data.";
      } else {
        text.textContent =
          "Claude Code hook installed. Run one Claude Code Desktop turn to create the cache.";
      }
      const repair = document.createElement("button");
      repair.type = "button";
      repair.className = "secondary-button";
      repair.textContent = "Repair hook";
      repair.addEventListener("click", async () => {
        repair.disabled = true;
        hookSettings.textContent = "Repairing Claude Code hook...";
        try {
          await invokeCommand("install_claude_rate_limits_hook");
          await loadHookSettings();
        } catch {
          hookSettings.textContent = "Claude Code hook repair failed";
        }
      });
      hookSettings.append(repair, text);
      return;
    }
    const button = document.createElement("button");
    button.type = "button";
    button.className = "secondary-button";
    button.textContent = "Enable Claude Code hook";
    button.addEventListener("click", async () => {
      button.disabled = true;
      hookSettings.textContent = "Enabling Claude Code hook...";
      try {
        await invokeCommand("install_claude_rate_limits_hook");
        await loadHookSettings();
      } catch {
        hookSettings.textContent = "Claude Code hook setup failed";
      }
    });
    const text = document.createElement("span");
    text.textContent = "Use Claude Code hook for fresher limits and fewer API calls";
    hookSettings.append(button, text);
  } catch {
    hookSettings.replaceChildren();
  }
}

refresh?.addEventListener("click", async () => {
  setStatus("Refreshing live usage...");
  try {
    render(await refreshLiveSnapshot());
    setStatus("Live usage connected");
  } catch {
    await load();
  }
});

void listenSnapshot(render).catch(() => undefined);
void load();
void loadHookSettings();
