import { describe, it, expect } from "vitest";
import { viewFor } from "./daemonUnavailable";

describe("viewFor", () => {
  it("maps store status to sub-view", () => {
    expect(viewFor("connecting")).toBe("connecting");
    expect(viewFor("disconnected")).toBe("disconnected");
    expect(viewFor("connected")).toBe("hidden");
  });
});
