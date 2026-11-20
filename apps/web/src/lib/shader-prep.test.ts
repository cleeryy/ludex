import { describe, expect, it } from "vitest";
import { isShaderPrepActive, shaderPrepHasCache, shaderPrepTone } from "./shader-prep";
import type { ShaderPrepState } from "./types";

const STATES: ShaderPrepState[] = ["never", "queued", "running", "done", "failed", "cancelled"];

describe("shader preparation", () => {
  it("only queued and running are active", () => {
    expect(STATES.filter(isShaderPrepActive)).toEqual(["queued", "running"]);
  });

  it("maps every state to a badge tone", () => {
    expect(shaderPrepTone("running")).toBe("signal");
    expect(shaderPrepTone("queued")).toBe("outline");
    expect(shaderPrepTone("done")).toBe("ok");
    expect(shaderPrepTone("failed")).toBe("danger");
    expect(shaderPrepTone("cancelled")).toBe("default");
    expect(shaderPrepTone("never")).toBe("default");
  });

  it("treats a missing or zero size as no cache", () => {
    expect(shaderPrepHasCache(0)).toBe(false);
    expect(shaderPrepHasCache(null)).toBe(false);
    expect(shaderPrepHasCache(undefined)).toBe(false);
    expect(shaderPrepHasCache(2048)).toBe(true);
  });
});
