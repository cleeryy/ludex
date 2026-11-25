// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PresetsBar, SurpriseMeButton } from "./presets-bar";

beforeEach(() => {
  window.localStorage.clear();
});

afterEach(cleanup);

describe("PresetsBar", () => {
  it("applies a built-in preset and clears it when clicked again", () => {
    const onApply = vi.fn();
    const onClear = vi.fn();
    const { rerender } = render(
      <PresetsBar
        params={new URLSearchParams("")}
        includeInstalled={true}
        hasFilters={false}
        onApply={onApply}
        onClear={onClear}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Unplayed backlog" }));
    expect(onApply).toHaveBeenCalledWith({ state: "unplayed", sort: "rating", order: "desc" });

    rerender(
      <PresetsBar
        params={new URLSearchParams("state=unplayed&sort=rating&order=desc")}
        includeInstalled={true}
        hasFilters={true}
        onApply={onApply}
        onClear={onClear}
      />,
    );
    const active = screen.getByRole("button", { name: "Unplayed backlog" });
    expect(active.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(active);
    expect(onClear).toHaveBeenCalledTimes(1);
  });

  it("saves and removes a custom filter preset", () => {
    render(
      <PresetsBar
        params={new URLSearchParams("players=2&minRating=80")}
        includeInstalled={false}
        hasFilters={true}
        onApply={vi.fn()}
        onClear={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Save view" }));
    fireEvent.change(screen.getByLabelText("Preset name"), { target: { value: "Co-op night" } });
    fireEvent.click(screen.getByRole("button", { name: "Save preset" }));

    expect(screen.getByRole("button", { name: "Co-op night" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Remove preset Co-op night" }));
    expect(screen.queryByRole("button", { name: "Co-op night" })).toBeNull();
  });

  it("renders SurpriseMeButton and fires onClick", () => {
    const onClick = vi.fn();
    render(<SurpriseMeButton loading={false} onClick={onClick} />);
    fireEvent.click(screen.getByRole("button", { name: "Surprise me" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
