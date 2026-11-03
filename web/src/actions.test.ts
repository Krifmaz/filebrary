import { describe, expect, it } from "vitest";
import { fileActions, registerAction } from "./actions";

const dialog = () => null;

describe("file actions", () => {
  it("orders by priority and replaces by id", () => {
    registerAction({ id: "a", label: "A", enabled: () => true, dialog });
    registerAction({ id: "b", label: "B", priority: 5, enabled: () => true, dialog });
    registerAction({ id: "a", label: "A2", priority: 9, enabled: () => true, dialog });
    expect(fileActions().map((x) => x.label)).toEqual(["A2", "B"]);
  });
});
