import assert from "node:assert/strict";
import test from "node:test";
import { surfaceCommand } from "../../qc-core/src/surface-actions.ts";
import { formFactors } from "../src/index.ts";

test("every hardware control role resolves to a surface command", () => {
  for (const formFactor of formFactors) {
    assert.ok(formFactor.controls.length > 0);
    for (const control of formFactor.controls) {
      const command = surfaceCommand({ kind: "switch", phase: "release", role: control.role });
      assert.notEqual(command.kind, "none", `${formFactor.id}: ${control.role} is a role the surface parser does not know`);
    }
  }
});

test("control roles are unique within a form factor", () => {
  for (const formFactor of formFactors) {
    const roles = formFactor.controls.map((control) => control.role);
    assert.equal(new Set(roles).size, roles.length, formFactor.id);
  }
});
