import { describe, expect, it } from "vitest";
import { bootstrapSchema } from "./bootstrap.fixture";
import { fields } from "./presentation";

describe("preference presentation", () => {
  it("presents exactly the keys the bootstrap schema knows", () => {
    expect(Object.keys(fields).sort()).toEqual(Object.keys(bootstrapSchema()).sort());
  });

  it("offers each selectable key's schema values, in the schema's order, and nothing else", () => {
    for (const [key, rule] of Object.entries(bootstrapSchema())) {
      const values = fields[key].options.map(option => option.value);
      const selectable = rule.values?.some(value => typeof value === "string");
      if (selectable) expect(values, key).toEqual(rule.values?.map(String));
      else expect(values, key).toEqual([]);
    }
  });
});
