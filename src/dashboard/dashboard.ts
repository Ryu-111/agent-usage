import "../shared/base.css";
import "./dashboard.css";
import {
  agentLabel,
  sourceLabel,
  windowLabel,
  type Agent,
  type AppSnapshot,
  type UsageHistory,
  type UsageSnapshot
} from "../shared/types";
import {
  invokeCommand,
  isBrowserPreview,
  listenSnapshot,
  loadLiveSnapshot,
  loadUsageHistory,
  previewHistory,
  previewSnapshot,
  refreshLiveSnapshot,
  unavailableSnapshot
} from "../shared/tauri";
import { dayKeyJst, formatDateTimeJst, formatDayJst, formatShortDayJst } from "../shared/time";

const rings = document.querySelector<HTMLDivElement>("#rings");
const burnRates = document.querySelector<HTMLDivElement>("#burn-rates");
const refresh = document.querySelector<HTMLButtonElement>("#refresh");
const status = document.querySelector<HTMLParagraphElement>("#status");
const hookSettings = document.querySelector<HTMLElement>("#hook-settings");
const webCookieSettings = document.querySelector<HTMLElement>("#web-cookie-settings");
const historyChart = document.querySelector<HTMLDivElement>("#history");
const historyLegend = document.querySelector<HTMLDivElement>("#history-legend");
const historyTooltip = document.querySelector<HTMLDivElement>("#history-tooltip");

const HISTORY_DAYS = 7;
const HISTORY_REFRESH_MS = 5 * 60_000;
const historyAgents: Agent[] = ["claudeCode", "codex"];
const compactNumber = new Intl.NumberFormat(undefined, {
  notation: "compact",
  maximumFractionDigits: 1
});

interface ClaudeHookStatus {
  installed: boolean;
  cacheExists: boolean;
  cacheFresh: boolean;
  cachePath: string | null;
  desktopTokensExists: boolean;
  desktopTokensFresh: boolean;
  desktopTokensPath: string | null;
  desktopTokensDate: string | null;
  desktopTokensModifiedAt: string | null;
  desktopBridgeEnabled: boolean;
  webCookieConfigured: boolean;
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

function formatDate(value: string | null): string {
  return value ?? "unknown date";
}

function formatDateTime(value: string | null): string {
  if (!value) {
    return "unknown time";
  }
  return formatDateTimeJst(value);
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

interface HistoryDay {
  key: string;
  label: string;
  shortLabel: string;
  totals: Record<Agent, number>;
}

/** Groups hourly buckets into the last HISTORY_DAYS calendar days in JST. */
function historyDays(data: UsageHistory, now = new Date()): HistoryDay[] {
  const days: HistoryDay[] = [];
  for (let offset = HISTORY_DAYS - 1; offset >= 0; offset -= 1) {
    // JST has no DST, so stepping back whole days always lands on the prior date.
    const date = new Date(now.getTime() - offset * 86_400_000);
    days.push({
      key: dayKeyJst(date),
      label: formatDayJst(date),
      shortLabel: formatShortDayJst(date),
      totals: { claudeCode: 0, codex: 0 }
    });
  }
  const byKey = new Map(days.map((day) => [day.key, day]));
  const start = new Date(data.bucketStart).getTime();
  const step = data.bucketMinutes * 60_000;
  for (const series of data.series) {
    series.tokens.forEach((tokens, index) => {
      const day = byKey.get(dayKeyJst(new Date(start + index * step)));
      if (day) {
        day.totals[series.agent] += tokens;
      }
    });
  }
  return days;
}

/** Rounds up to a value whose half is also readable (e.g. 300K → 150K). */
function niceCeiling(value: number): number {
  if (value <= 0) {
    return 1;
  }
  const magnitude = 10 ** Math.floor(Math.log10(value));
  const step =
    [1, 1.2, 1.5, 2, 3, 4, 5, 6, 8, 10].find((candidate) => candidate * magnitude >= value) ?? 10;
  return step * magnitude;
}

function renderHistoryLegend(): void {
  historyLegend?.replaceChildren(
    ...historyAgents.map((agent) => {
      const item = document.createElement("span");
      item.className = "legend-item";
      item.innerHTML = `<i class="swatch" data-agent="${agent}"></i>${agentLabel[agent]}`;
      return item;
    })
  );
}

function showHistoryTooltip(day: HistoryDay, anchor: HTMLElement): void {
  if (!historyTooltip) {
    return;
  }
  const total = historyAgents.reduce((sum, agent) => sum + day.totals[agent], 0);
  historyTooltip.innerHTML = `
    <strong>${day.label}</strong>
    ${historyAgents
      .map(
        (agent) => `
          <span class="tooltip-row">
            <i class="swatch" data-agent="${agent}"></i>${agentLabel[agent]}
            <b>${day.totals[agent].toLocaleString()}</b>
          </span>`
      )
      .join("")}
    <span class="tooltip-row tooltip-total">Total<b>${total.toLocaleString()}</b></span>
  `;
  historyTooltip.hidden = false;
  const panel = historyTooltip.offsetParent as HTMLElement | null;
  if (!panel) {
    return;
  }
  const panelRect = panel.getBoundingClientRect();
  const anchorRect = anchor.getBoundingClientRect();
  const width = historyTooltip.offsetWidth;
  const center = anchorRect.left - panelRect.left + anchorRect.width / 2;
  const left = Math.min(Math.max(center - width / 2, 8), panelRect.width - width - 8);
  historyTooltip.style.left = `${left}px`;
  historyTooltip.style.top = `${anchorRect.top - panelRect.top - historyTooltip.offsetHeight - 8}px`;
}

function hideHistoryTooltip(): void {
  if (historyTooltip) {
    historyTooltip.hidden = true;
  }
}

function renderHistory(data: UsageHistory): void {
  if (!historyChart) {
    return;
  }
  const days = historyDays(data);
  const peak = Math.max(0, ...days.flatMap((day) => historyAgents.map((agent) => day.totals[agent])));
  if (peak === 0) {
    const empty = document.createElement("p");
    empty.className = "history-empty";
    empty.textContent = "No local token usage in the last 7 days";
    historyChart.replaceChildren(empty);
    return;
  }

  const ceiling = niceCeiling(peak);
  const axis = document.createElement("div");
  axis.className = "history-axis";
  const plot = document.createElement("div");
  plot.className = "history-plot";
  const gridLayer = document.createElement("div");
  gridLayer.className = "history-grid";
  plot.append(gridLayer);
  for (const fraction of [1, 0.5, 0]) {
    const label = document.createElement("span");
    label.style.setProperty("--y", `${fraction * 100}%`);
    label.textContent = compactNumber.format(ceiling * fraction);
    axis.append(label);
    const line = document.createElement("i");
    line.className = fraction === 0 ? "gridline baseline" : "gridline";
    line.style.setProperty("--y", `${fraction * 100}%`);
    gridLayer.append(line);
  }

  const today = dayKeyJst(new Date());
  const columns = days.map((day) => {
    const column = document.createElement("div");
    column.className = "history-day";
    column.tabIndex = 0;
    column.setAttribute(
      "aria-label",
      `${day.label}: ${historyAgents
        .map((agent) => `${agentLabel[agent]} ${day.totals[agent].toLocaleString()} tokens`)
        .join(", ")}`
    );
    const bars = document.createElement("div");
    bars.className = "history-bars";
    for (const agent of historyAgents) {
      const bar = document.createElement("span");
      bar.className = "history-bar";
      bar.dataset.agent = agent;
      bar.style.setProperty("--h", `${(day.totals[agent] / ceiling) * 100}%`);
      bars.append(bar);
    }
    const label = document.createElement("span");
    label.className = day.key === today ? "history-label is-today" : "history-label";
    label.textContent = day.shortLabel;
    column.append(bars, label);
    column.addEventListener("mouseenter", () => showHistoryTooltip(day, bars));
    column.addEventListener("focus", () => showHistoryTooltip(day, bars));
    column.addEventListener("mouseleave", hideHistoryTooltip);
    column.addEventListener("blur", hideHistoryTooltip);
    return column;
  });
  const grid = document.createElement("div");
  grid.className = "history-days";
  grid.append(...columns);
  plot.append(grid);
  historyChart.replaceChildren(axis, plot);
}

async function loadHistory(): Promise<void> {
  try {
    renderHistory(await loadUsageHistory());
  } catch {
    if (isBrowserPreview()) {
      renderHistory(previewHistory());
    } else if (historyChart) {
      const empty = document.createElement("p");
      empty.className = "history-empty";
      empty.textContent = "History unavailable";
      historyChart.replaceChildren(empty);
    }
  }
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
        text.textContent = `Claude Desktop token cache stale (${formatDate(
          hookStatus.desktopTokensDate,
        )}, updated ${formatDateTime(hookStatus.desktopTokensModifiedAt)})`;
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

async function loadWebCookieSettings(): Promise<void> {
  if (!webCookieSettings || isBrowserPreview()) {
    webCookieSettings?.replaceChildren();
    return;
  }
  try {
    const hookStatus = await invokeCommand<ClaudeHookStatus>("get_claude_rate_limits_hook_status");
    webCookieSettings.replaceChildren();
    const input = document.createElement("input");
    input.type = "password";
    input.placeholder = "Claude sessionKey cookie";
    input.autocomplete = "off";
    input.className = "cookie-input";
    const save = document.createElement("button");
    save.type = "button";
    save.className = "secondary-button";
    save.textContent = hookStatus.webCookieConfigured ? "Replace Web cookie" : "Set Web cookie";
    save.addEventListener("click", async () => {
      if (!input.value.trim()) return;
      save.disabled = true;
      try {
        await invokeCommand("set_claude_web_cookie", { cookie: input.value.trim() });
        input.value = "";
        await loadWebCookieSettings();
      } catch {
        save.disabled = false;
        save.textContent = "Web cookie failed";
      }
    });
    webCookieSettings.append(save);
    if (hookStatus.webCookieConfigured) {
      const clear = document.createElement("button");
      clear.type = "button";
      clear.className = "secondary-button";
      clear.textContent = "Clear Web cookie";
      clear.addEventListener("click", async () => {
        await invokeCommand("clear_claude_web_cookie");
        await loadWebCookieSettings();
      });
      webCookieSettings.append(clear);
    }
    const text = document.createElement("span");
    text.textContent = hookStatus.webCookieConfigured
      ? "Web fallback cookie stored in macOS Keychain"
      : "Optional Claude Web fallback; cookie value stays in macOS Keychain";
    webCookieSettings.append(input, text);
  } catch {
    webCookieSettings.replaceChildren();
  }
}

refresh?.addEventListener("click", async () => {
  setStatus("Refreshing live usage...");
  void loadHistory();
  try {
    render(await refreshLiveSnapshot());
    setStatus("Live usage connected");
  } catch {
    await load();
  }
});

void listenSnapshot(render).catch(() => undefined);
void load();
renderHistoryLegend();
void loadHistory();
window.setInterval(() => void loadHistory(), HISTORY_REFRESH_MS);
void loadHookSettings();
void loadWebCookieSettings();
