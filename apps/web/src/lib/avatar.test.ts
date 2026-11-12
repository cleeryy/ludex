import { describe, expect, it } from "vitest";
import { AVATAR_MAX_BYTES, avatarFileProblem } from "@/lib/avatar-file";

function file(type: string, size: number): File {
  return new File([new Uint8Array(size)], "logo", { type });
}

describe("avatarFileProblem", () => {
  it("accepts the three supported formats under the cap", () => {
    expect(avatarFileProblem(file("image/png", 1024))).toBeNull();
    expect(avatarFileProblem(file("image/jpeg", AVATAR_MAX_BYTES))).toBeNull();
    expect(avatarFileProblem(file("image/webp", 256 * 1024))).toBeNull();
  });

  it("rejects every other type", () => {
    expect(avatarFileProblem(file("image/gif", 1024))).toBe("type");
    expect(avatarFileProblem(file("image/svg+xml", 1024))).toBe("type");
    expect(avatarFileProblem(file("text/html", 1024))).toBe("type");
    expect(avatarFileProblem(file("", 1024))).toBe("type");
  });

  it("rejects empty and oversized files", () => {
    expect(avatarFileProblem(file("image/png", 0))).toBe("type");
    expect(avatarFileProblem(file("image/png", AVATAR_MAX_BYTES + 1))).toBe("size");
  });
});
