const TIME_ZONE = "Asia/Tokyo";

const clockFormat = new Intl.DateTimeFormat("ja-JP", {
  timeZone: TIME_ZONE,
  hour: "2-digit",
  minute: "2-digit",
  hour12: false
});

const dateTimeFormat = new Intl.DateTimeFormat("ja-JP", {
  timeZone: TIME_ZONE,
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false
});

export function formatClockJst(value: string | Date): string {
  return clockFormat.format(new Date(value));
}

export function formatDateTimeJst(value: string | Date): string {
  return `${dateTimeFormat.format(new Date(value))} JST`;
}

const dayKeyFormat = new Intl.DateTimeFormat("en-CA", {
  timeZone: TIME_ZONE,
  year: "numeric",
  month: "2-digit",
  day: "2-digit"
});

const dayLabelFormat = new Intl.DateTimeFormat("ja-JP", {
  timeZone: TIME_ZONE,
  month: "numeric",
  day: "numeric",
  weekday: "short"
});

/** Calendar day in JST as YYYY-MM-DD, for grouping timestamps by day. */
export function dayKeyJst(value: string | Date): string {
  return dayKeyFormat.format(new Date(value));
}

export function formatDayJst(value: string | Date): string {
  return dayLabelFormat.format(new Date(value));
}

const shortDayFormat = new Intl.DateTimeFormat("ja-JP", {
  timeZone: TIME_ZONE,
  month: "numeric",
  day: "numeric"
});

export function formatShortDayJst(value: string | Date): string {
  return shortDayFormat.format(new Date(value));
}
