import { describe, expect, it } from "vitest";
import {
  POWER_CONFIRM_CANCEL_INDEX,
  POWER_GRID_COLUMNS,
  needsPowerConfirm,
  powerActions,
  powerConfirmActive,
  powerGridStep,
  resolvePowerConfirm,
  type PowerContext,
} from "./power";

/* --------------------------------------------------------------------------
   #196: the Power surface and its confirmation. The default has to be Cancel,
   and a browser build must never pretend it can sleep or quit a process.
   -------------------------------------------------------------------------- */

const DESKTOP: PowerContext = { console: false, native: true, gameTitle: null };
const CONSOLE: PowerContext = { console: true, native: true, gameTitle: null };
const BROWSER: PowerContext = { console: false, native: false, gameTitle: null };

function actionIds(context: PowerContext): string[] {
  return powerActions(context).map((action) => action.id);
}

describe("power confirmation", () => {
  it("starts on Cancel so a reflex A never quits", () => {
    expect(powerConfirmActive()).toBe(POWER_CONFIRM_CANCEL_INDEX);
    expect(POWER_CONFIRM_CANCEL_INDEX).toBe(1);
    expect(resolvePowerConfirm(powerConfirmActive())).toBe("cancel");
    expect(resolvePowerConfirm(0)).toBe("confirm");
  });

  it("only asks again for actions that end something", () => {
    expect(needsPowerConfirm("exit")).toBe(false);
    for (const id of ["end-session", "sleep", "restart", "quit", "quit-game"] as const) {
      expect(needsPowerConfirm(id)).toBe(true);
    }
  });
});

describe("power actions by platform", () => {
  it("offers End session in a console session and hides Exit Big Picture", () => {
    const actions = powerActions(CONSOLE);
    expect(actionIds(CONSOLE)).toEqual(["end-session", "sleep", "restart", "quit"]);
    expect(actions.every((action) => action.enabled)).toBe(true);
    expect(actions.find((action) => action.id === "quit")?.detail).toBe("Quit to desktop. Downloads stop.");
  });

  it("offers a desktop quit that tells the truth about downloads", () => {
    const quit = powerActions(DESKTOP).find((action) => action.id === "quit");
    expect(quit?.detail).toBe("Close Ludex. Downloads stop.");
    expect(quit?.tone).toBe("danger");
    expect(actionIds(DESKTOP)).toContain("exit");
    expect(actionIds(DESKTOP)).not.toContain("end-session");
  });

  it("keeps browser actions visible but disabled with a reason", () => {
    const actions = powerActions(BROWSER);
    expect(actionIds(BROWSER)).toEqual(["sleep", "restart", "quit", "exit"]);
    for (const action of actions.filter((entry) => entry.id !== "exit")) {
      expect(action.enabled).toBe(false);
      expect(action.reason).toBe("Only available in the desktop app.");
    }
    expect(actions.find((action) => action.id === "exit")?.enabled).toBe(true);
  });

  it("only adds Quit game when Big Picture launched a game", () => {
    const withGame: PowerContext = { ...DESKTOP, gameTitle: "Hades II" };
    const action = powerActions(withGame).find((entry) => entry.id === "quit-game");
    expect(action?.label).toBe("Quit Hades II");
    expect(action?.detail).toBe("Close Hades II and return to Big Picture.");
    expect(action?.enabled).toBe(true);
    expect(actionIds(DESKTOP)).not.toContain("quit-game");
  });
});

describe("power grid geometry", () => {
  it("uses two columns, matching the rendered tiles", () => {
    expect(POWER_GRID_COLUMNS).toBe(2);
    expect(powerGridStep(0, "down", 4)).toBe(2);
    expect(powerGridStep(3, "up", 4)).toBe(1);
    expect(powerGridStep(1, "right", 4)).toBe(2);
    expect(powerGridStep(0, "left", 4)).toBe(0);
    expect(powerGridStep(3, "down", 4)).toBe(3);
  });
});
