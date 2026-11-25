// @vitest-environment jsdom
import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ArtPlate, GameBackdrop, GameCover, resolveArtCandidates } from "./cover";

afterEach(cleanup);

describe("resolveArtCandidates", () => {
  it("deduplicates real artwork URLs and skips empty or demo placeholders", () => {
    expect(
      resolveArtCandidates(null, ["demo://art/1", "/api/media/shot.jpg", "/api/media/shot.jpg", "/api/media/cover.jpg"]),
    ).toEqual(["/api/media/shot.jpg", "/api/media/cover.jpg"]);
  });

  it("keeps primary artwork first", () => {
    expect(resolveArtCandidates("/api/media/bg.jpg", ["/api/media/shot.jpg", "/api/media/cover.jpg"])).toEqual([
      "/api/media/bg.jpg",
      "/api/media/shot.jpg",
      "/api/media/cover.jpg",
    ]);
  });
});

describe("GameCover", () => {
  it("renders artwork full-bleed with object-cover and never applies blurred letterboxing", () => {
    const { container } = render(<GameCover title="Hades" coverUrl="/api/media/hades-portrait.jpg" />);
    const img = container.querySelector("img") as HTMLImageElement;
    expect(img).toBeTruthy();
    Object.defineProperty(img, "naturalWidth", { value: 460, configurable: true });
    Object.defineProperty(img, "naturalHeight", { value: 215, configurable: true });
    fireEvent.load(img);
    expect(container.querySelectorAll("img").length).toBe(1);
    expect(container.querySelector('img[data-cover-fit="cover"]')).toBeTruthy();
    expect(container.querySelector('img[data-cover-fit="contain"]')).toBeNull();
  });

  it("supports wide 16:9 variant and steps through fallbackUrl on load error", () => {
    const { container } = render(
      <GameCover
        title="Hades"
        variant="wide"
        coverUrl="/api/media/missing-hero.jpg"
        fallbackUrl="/api/media/hades-cover.jpg"
      />,
    );
    expect(container.querySelector(".cover.cover-wide")).toBeTruthy();
    const first = container.querySelector("img") as HTMLImageElement;
    expect(first?.getAttribute("src")).toBe("/api/media/missing-hero.jpg");
    fireEvent.error(first);
    const second = container.querySelector("img") as HTMLImageElement;
    expect(second?.getAttribute("src")).toBe("/api/media/hades-cover.jpg");
  });

  it("falls back to the procedural motif when the cover image fails to load", () => {
    const { container } = render(<GameCover title="Broken" coverUrl="/api/media/missing.jpg" />);
    const img = container.querySelector("img") as HTMLImageElement;
    fireEvent.error(img);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("svg.cover-motif")).toBeTruthy();
  });
});

describe("ArtPlate and GameBackdrop", () => {
  it("uses fallback artwork when backgroundUrl is missing and steps through broken URLs", () => {
    const { container } = render(
      <ArtPlate
        title="Hades"
        artUrl={null}
        fallbackUrls={["/api/media/shot.jpg", "/api/media/cover.jpg"]}
      />,
    );
    const first = container.querySelector("img") as HTMLImageElement;
    expect(first?.getAttribute("src")).toBe("/api/media/shot.jpg");

    fireEvent.error(first);
    const second = container.querySelector("img") as HTMLImageElement;
    expect(second?.getAttribute("src")).toBe("/api/media/cover.jpg");

    fireEvent.error(second);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("svg")).toBeTruthy();
  });

  it("falls back on GameBackdrop when the primary background fails", () => {
    const { container } = render(
      <GameBackdrop
        title="Hades"
        backgroundUrl="/api/media/missing-bg.jpg"
        fallbackUrls={["/api/media/cover.jpg"]}
      />,
    );
    const first = container.querySelector("img") as HTMLImageElement;
    expect(first?.getAttribute("src")).toBe("/api/media/missing-bg.jpg");

    fireEvent.error(first);
    const fallback = container.querySelector("img") as HTMLImageElement;
    expect(fallback?.getAttribute("src")).toBe("/api/media/cover.jpg");
  });
});
