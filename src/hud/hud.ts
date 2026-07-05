import "../shared/base.css";
import "./hud.css";
import { agentLabel, windowLabel, type AppSnapshot, type UsageSnapshot } from "../shared/types";
import {
  invokeCommand,
  isBrowserPreview,
  listenSnapshot,
  loadLiveSnapshot,
  previewSnapshot,
  unavailableSnapshot
} from "../shared/tauri";

const hud = document.querySelector<HTMLElement>("#hud");
const rows = document.querySelector<HTMLDivElement>("#rows");
const updated = document.querySelector<HTMLSpanElement>("#updated");
const opacity = document.querySelector<HTMLInputElement>("#opacity");
const opacityDown = document.querySelector<HTMLButtonElement>("#opacity-down");
const openDashboard = document.querySelector<HTMLButtonElement>("#open-dashboard");
const minimize = document.querySelector<HTMLButtonElement>("#minimize");

const HUD_SIZE = {
  expanded: { width: 360, height: 330 }
};

function percent(snapshot: UsageSnapshot): number {
  if (typeof snapshot.utilizationPct === "number") {
    return Math.max(0, Math.min(100, snapshot.utilizationPct));
  }
  const ceiling = snapshot.window === "fiveHour" ? 250_000 : 1_250_000;
  return Math.max(0, Math.min(100, ((snapshot.usedTokens ?? 0) / ceiling) * 100));
}

function formatReset(value: string | null): string {
  if (!value) {
    return "Reset --";
  }
  const resetAt = new Date(value);
  const minutes = Math.max(0, Math.round((resetAt.getTime() - Date.now()) / 60_000));
  const clock = resetAt.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (minutes >= 1440) {
    return `Reset ${clock} · ${Math.round(minutes / 1440)}d`;
  }
  if (minutes >= 60) {
    return `Reset ${clock} · ${Math.round(minutes / 60)}h`;
  }
  return `Reset ${clock} · ${minutes}m`;
}

function formatDetail(snapshot: UsageSnapshot): string {
  if (snapshot.source === "unavailable") {
    return "Unavailable";
  }
  return formatReset(snapshot.resetAt);
}

function render(snapshot: AppSnapshot): void {
  if (!rows || !updated) {
    return;
  }
  updated.textContent = new Date(snapshot.capturedAt).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit"
  });

  const usageByWindow = snapshot.agents.flatMap((agent) =>
    agent.windows.map((usage) => ({ ...usage, agent: agent.agent }))
  );
  const compact = [
    ...usageByWindow.filter((usage) => usage.window === "fiveHour"),
    ...usageByWindow.filter((usage) => usage.window === "weekly")
  ];

  rows.replaceChildren(
    ...compact.map((usage, index) => {
      const row = document.createElement("div");
      const pct = percent(usage);
      row.className = "usage-row";
      if (index > 0 && compact[index - 1].window !== usage.window) {
        row.classList.add("starts-weekly");
      }
      row.innerHTML = `
        <span>${agentLabel[usage.agent]} ${windowLabel[usage.window]}</span>
        <strong>${Math.round(pct)}%</strong>
        <small>${formatDetail(usage)}</small>
        <i style="--pct: ${pct}%"></i>
      `;
      return row;
    })
  );
}

function applyOpacity(value: number): void {
  const normalized = Math.max(30, Math.min(100, value));
  document.documentElement.style.setProperty("--hud-opacity", String(normalized / 100));
  localStorage.setItem("agentUsageHudOpacity", String(normalized));
  if (opacity) {
    opacity.value = String(normalized);
  }
}

async function setHudSize(): Promise<void> {
  try {
    const { getCurrentWindow, LogicalSize } = await import("@tauri-apps/api/window");
    const size = HUD_SIZE.expanded;
    await getCurrentWindow().setSize(new LogicalSize(size.width, size.height));
  } catch {
    // Browser preview keeps the CSS state even without Tauri window APIs.
  }
}

async function minimizeHud(): Promise<void> {
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().minimize();
  } catch {
    undefined;
  }
}

async function startDrag(event: MouseEvent): Promise<void> {
  if (event.button !== 0 || (event.target as HTMLElement).closest("button,input,.no-drag")) {
    return;
  }
  event.preventDefault();
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().startDragging();
  } catch {
    undefined;
  }
}

openDashboard?.addEventListener("click", () => {
  void invokeCommand("show_dashboard").catch(() => undefined);
});

minimize?.addEventListener("click", () => {
  void minimizeHud();
});

opacity?.addEventListener("input", () => {
  applyOpacity(Number(opacity.value));
});

opacityDown?.addEventListener("click", () => {
  applyOpacity(Number(opacity?.value ?? 82) <= 35 ? 82 : 35);
});

hud?.addEventListener("mousedown", (event) => {
  void startDrag(event);
});

async function load(): Promise<void> {
  if (updated) {
    updated.textContent = "Loading";
  }
  try {
    render(await loadLiveSnapshot());
  } catch {
    render(isBrowserPreview() ? previewSnapshot() : unavailableSnapshot());
  }
}

applyOpacity(Number(localStorage.getItem("agentUsageHudOpacity") ?? 82));
void setHudSize();
void listenSnapshot(render).catch(() => undefined);
void load();
