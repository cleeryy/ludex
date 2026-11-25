// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { HeroSynopsis } from "./hero-synopsis";

afterEach(cleanup);

describe("HeroSynopsis", () => {
  it("does not show an expand toggle for short descriptions", () => {
    render(<HeroSynopsis description="Defy the god of the dead as you hack and slash out of the Underworld." />);
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("clamps long synopses and toggles between Read more and Show less", () => {
    const longDescription =
      "An expansive sci-fi tactical RPG where your crew explores derelict megastructures across fractured star systems, negotiates uneasy alliances with rival salvage syndicates, upgrades modular ships, and uncovers the hidden history behind the collapse of the frontier network.";
    const { container } = render(<HeroSynopsis description={longDescription} />);

    const paragraph = container.querySelector("p");
    expect(paragraph?.className).toContain("line-clamp-3");

    const toggle = screen.getByRole("button", { name: "Read more" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");

    fireEvent.click(toggle);
    expect(paragraph?.className).not.toContain("line-clamp-3");
    const collapse = screen.getByRole("button", { name: "Show less" });
    expect(collapse.getAttribute("aria-expanded")).toBe("true");

    fireEvent.click(collapse);
    expect(paragraph?.className).toContain("line-clamp-3");
  });
});
