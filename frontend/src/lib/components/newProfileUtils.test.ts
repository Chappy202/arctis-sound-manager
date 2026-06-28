import { describe, it, expect } from "vitest";
import { validateProfileName } from "./newProfileUtils.js";

describe("validateProfileName", () => {
  // ── empty / whitespace ──────────────────────────────────────────────────
  it("rejects empty string", () => {
    const r = validateProfileName("", []);
    expect(r.ok).toBe(false);
    expect(r.name).toBe("");
    expect(r.error).toBe("Name required");
  });

  it("rejects whitespace-only string", () => {
    const r = validateProfileName("   ", []);
    expect(r.ok).toBe(false);
    expect(r.name).toBe("");
    expect(r.error).toBe("Name required");
  });

  // ── length ───────────────────────────────────────────────────────────────
  it("rejects names longer than 48 chars after trimming", () => {
    const long = "a".repeat(49);
    const r = validateProfileName(long, []);
    expect(r.ok).toBe(false);
    expect(r.name).toBe(long); // trimmed (no surrounding whitespace)
    expect(r.error).toBe("Name too long (max 48)");
  });

  it("accepts exactly 48-char name", () => {
    const exact = "a".repeat(48);
    const r = validateProfileName(exact, []);
    expect(r.ok).toBe(true);
    expect(r.name).toBe(exact);
    expect(r.error).toBeUndefined();
  });

  // ── duplicate check ──────────────────────────────────────────────────────
  it("rejects exact duplicate (case-sensitive)", () => {
    const r = validateProfileName("Gaming", ["Gaming", "Default"]);
    expect(r.ok).toBe(false);
    expect(r.name).toBe("Gaming");
    expect(r.error).toBe(`A profile named "Gaming" already exists`);
  });

  it("accepts a name that differs only in case", () => {
    const r = validateProfileName("gaming", ["Gaming", "Default"]);
    expect(r.ok).toBe(true);
    expect(r.name).toBe("gaming");
  });

  it("accepts a new unique name", () => {
    const r = validateProfileName("Streaming", ["Gaming", "Default"]);
    expect(r.ok).toBe(true);
    expect(r.name).toBe("Streaming");
    expect(r.error).toBeUndefined();
  });

  // ── trimming ─────────────────────────────────────────────────────────────
  it("trims surrounding whitespace before validation", () => {
    const r = validateProfileName("  Gaming  ", ["Default"]);
    expect(r.ok).toBe(true);
    expect(r.name).toBe("Gaming");
  });

  it("trims before duplicate check", () => {
    const r = validateProfileName("  Gaming  ", ["Gaming"]);
    expect(r.ok).toBe(false);
    expect(r.name).toBe("Gaming");
    expect(r.error).toBe(`A profile named "Gaming" already exists`);
  });
});
