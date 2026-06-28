import { describe, it, expect } from "vitest";
import { pillTitle } from "./appPillUtils";

describe("pillTitle", () => {
  it("returns app_name when media_name is null", () => {
    const stream = { app_name: "Firefox", media_name: null };
    expect(pillTitle(stream)).toBe("Firefox");
  });

  it("returns app_name when media_name is empty string", () => {
    const stream = { app_name: "Firefox", media_name: "" };
    expect(pillTitle(stream)).toBe("Firefox");
  });

  it("returns app_name when media_name is whitespace only", () => {
    const stream = { app_name: "Firefox", media_name: "  \t\n  " };
    expect(pillTitle(stream)).toBe("Firefox");
  });

  it("returns formatted string with both when media_name is distinct", () => {
    const stream = { app_name: "Firefox", media_name: "YouTube" };
    expect(pillTitle(stream)).toBe("Firefox — YouTube");
  });

  it("returns app_name when media_name equals app_name (no duplication)", () => {
    const stream = { app_name: "Firefox", media_name: "Firefox" };
    expect(pillTitle(stream)).toBe("Firefox");
  });

  it("returns full long app_name (recourse for truncated pill)", () => {
    const longName = "Very Long Application Name That Gets Truncated";
    const stream = { app_name: longName, media_name: null };
    expect(pillTitle(stream)).toBe(longName);
  });
});
