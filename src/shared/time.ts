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
