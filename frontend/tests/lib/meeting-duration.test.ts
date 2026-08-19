import { describe, expect, test } from "bun:test";
import { formatMeetingDuration } from "../../src/lib/meetingDuration";

describe("formatMeetingDuration", () => {
  test("formats sub-minute recordings in seconds", () => {
    expect(formatMeetingDuration(42)).toBe("42s");
    expect(formatMeetingDuration(59.4)).toBe("59s");
  });

  test("formats sub-hour recordings in whole minutes", () => {
    expect(formatMeetingDuration(60)).toBe("1m");
    expect(formatMeetingDuration(17 * 60 + 20)).toBe("17m");
  });

  test("formats an hour or more as h:mm", () => {
    expect(formatMeetingDuration(2 * 3600 + 35 * 60)).toBe("2:35h");
    expect(formatMeetingDuration(3600 + 5 * 60)).toBe("1:05h");
    // Rounds up across the hour boundary instead of reporting "60m".
    expect(formatMeetingDuration(59 * 60 + 40)).toBe("1:00h");
  });

  test("returns null when there is no usable duration", () => {
    expect(formatMeetingDuration(undefined)).toBeNull();
    expect(formatMeetingDuration(null)).toBeNull();
    expect(formatMeetingDuration(0)).toBeNull();
    expect(formatMeetingDuration(-5)).toBeNull();
    expect(formatMeetingDuration(NaN)).toBeNull();
  });
});
