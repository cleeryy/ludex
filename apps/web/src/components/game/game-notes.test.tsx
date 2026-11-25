// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { loadGameNote } from "@/lib/game-notes";
import { GameNotesPanel } from "./game-notes";

beforeEach(() => {
  window.localStorage.clear();
});

afterEach(cleanup);

describe("GameNotesPanel", () => {
  it("saves a note on blur and clears it on Clear note", () => {
    render(<GameNotesPanel gameId={12} />);
    const textarea = screen.getByLabelText("Personal notes");

    fireEvent.change(textarea, { target: { value: "Boss code: 4815" } });
    fireEvent.blur(textarea);

    expect(loadGameNote(12)?.body).toBe("Boss code: 4815");

    fireEvent.click(screen.getByRole("button", { name: "Clear note" }));
    expect(loadGameNote(12)).toBeNull();
  });
});
