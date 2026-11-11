import { describe, expect, it } from "vitest";
import { CARD_CUE, CARD_STAGGER, CARD_STAGGER_CAP, ENTRANCE_MS, cardDelay, reveal } from "./entrance";

describe("big picture entrance", () => {
  it("staggers cards between 20 and 40ms and caps the delay", () => {
    expect(CARD_STAGGER * 1000).toBeGreaterThanOrEqual(20);
    expect(CARD_STAGGER * 1000).toBeLessThanOrEqual(40);
    expect(cardDelay(0)).toBeLessThan(cardDelay(3));
    expect(cardDelay(CARD_STAGGER_CAP)).toBe(cardDelay(CARD_STAGGER_CAP + 12));
  });

  it("finishes the whole cascade under 450ms", () => {
    expect(ENTRANCE_MS).toBeLessThan(450);
    expect(ENTRANCE_MS).toBeGreaterThanOrEqual(400);
  });

  it("moves and fades when motion is allowed", () => {
    const cue = reveal(false, { y: 10, delay: 0.05, duration: 0.28 });
    expect(cue.initial).toEqual({ opacity: 0, y: 10, scale: 1 });
    expect(cue.animate).toEqual({ opacity: 1, y: 0, scale: 1 });
    expect(cue.transition).toMatchObject({ delay: 0.05, duration: 0.28 });
  });

  it("fades in place under prefers-reduced-motion", () => {
    const cue = reveal(true, { y: 14, scale: 0.985, delay: 0.08, duration: 0.3 });
    expect(cue.initial).toEqual({ opacity: 0 });
    expect(cue.animate).toEqual({ opacity: 1 });
    expect(cue.transition).toMatchObject({ duration: 0.12 });
    expect(cue.transition).not.toHaveProperty("delay");
    expect(cue.initial).not.toHaveProperty("y");
    expect(cue.initial).not.toHaveProperty("scale");
  });

  it("mounts settled once the entrance has played", () => {
    expect(reveal(false, { settled: true, y: 8 })).toMatchObject({ initial: false });
  });

  it("keeps the backdrop at its final opacity", () => {
    const cue = reveal(false, { opacity: 0.25, duration: 0.42 });
    expect(cue.initial).toEqual({ opacity: 0, y: 0, scale: 1 });
    expect(cue.animate).toEqual({ opacity: 0.25, y: 0, scale: 1 });
    expect(cue.transition).toMatchObject({ duration: 0.42 });
  });

  it("uses the card cue for the shelf", () => {
    expect(CARD_CUE.y).toBeLessThanOrEqual(10);
    expect(cardDelay(0)).toBe(CARD_CUE.delay);
  });
});
