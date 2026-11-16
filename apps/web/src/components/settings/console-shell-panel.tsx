import { useNavigate } from "react-router-dom";
import { t } from "@/lib/i18n";
import { CONSOLE_ROOT } from "@/components/console/routes/surface";
import { setConsoleShellEnabled, useConsoleShellFlag } from "@/lib/console-flag";
import { Button } from "@/components/ui/controls";
import { SettingsGroup, SettingsSwitchRow } from "@/components/settings/settings-section";

/**
 * The `console_v2` device flag, where a machine is configured (#267).
 *
 * It lives in Settings → Help and about ("About this app"), next to the version
 * and the device controls, and never in the account preference document: the
 * question is "does this television use the console shell?", not "what does
 * this account prefer?". Phase 1 defaults it off; the route itself stays a
 * deep link either way.
 */
export function ConsoleShellPanel() {
  const enabled = useConsoleShellFlag();
  const navigate = useNavigate();
  return (
    <SettingsGroup
      title={t("Console shell")}
      description={t("A device choice: open the console shell instead of the desktop home on this machine.")}
      actions={
        <Button variant="default" size="sm" onClick={() => navigate(CONSOLE_ROOT)}>
          {t("Open the console")}
        </Button>
      }
    >
      <SettingsSwitchRow
        title={t("Use the console shell on this device")}
        description={t("Stored on this device only. Off keeps the desktop home; the console is still one link away.")}
        checked={enabled}
        onCheckedChange={setConsoleShellEnabled}
      />
    </SettingsGroup>
  );
}
