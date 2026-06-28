/**
 * selectUtils.test.ts — TDD suite for the Select trigger label helper.
 *
 * Pure-logic tests only (Node env, no DOM). Vitest default runner.
 */
import { describe, expect, it } from "vitest";
import { selectedLabel, type SelectOption } from "./selectUtils.js";

describe("selectedLabel", () => {
  it("returns the label for a matching value", () => {
    const options: SelectOption[] = [
      { value: "a", label: "Option A" },
      { value: "b", label: "Option B" },
    ];
    expect(selectedLabel(options, "a")).toBe("Option A");
    expect(selectedLabel(options, "b")).toBe("Option B");
  });

  it("falls back to the raw value when no option matches", () => {
    const options: SelectOption[] = [{ value: "a", label: "Option A" }];
    expect(selectedLabel(options, "z")).toBe("z");
  });

  it("falls back to the raw value for empty options", () => {
    expect(selectedLabel([], "x")).toBe("x");
    expect(selectedLabel([], "")).toBe("");
  });

  it("returns the first match for duplicate values", () => {
    const options: SelectOption[] = [
      { value: "a", label: "First A" },
      { value: "a", label: "Second A" },
    ];
    expect(selectedLabel(options, "a")).toBe("First A");
  });
});
