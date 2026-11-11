import { useCallback, useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { installsAvailable } from "@/lib/install";
import { isNative } from "@/lib/native";
import { t } from "@/lib/i18n";
import {
  availableSettingsSections,
  resolveSettingsSection,
  settingsSectionPath,
  type SettingsSectionId,
} from "@/lib/settings-sections";
import { useAuth } from "@/state/auth";
import { PageHeader } from "@/components/page";
import { LanguagePanel } from "@/components/settings/language-panel";
import { SettingsNav } from "@/components/settings/settings-nav";
import { SettingsSection } from "@/components/settings/settings-section";
import {
  AccountBody,
  AppearanceBody,
  HelpBody,
  ProfilesBody,
} from "@/components/settings/sections-account";
import {
  CompatibilityBody,
  DownloadsBody,
  IntegrationsBody,
  SavesBody,
  StorageBody,
  StreamingBody,
  UpdatesBody,
} from "@/components/settings/sections-device";
import { useDesktopSettings } from "@/components/settings/use-desktop-settings";
import { settingsSectionDescription, settingsSectionLabel } from "@/components/settings/settings-copy";

/**
 * Settings (#174): one page, one category at a time.
 *
 * The category is a URL (`?section=`), the nav is a list of links, and the
 * heading takes focus when a link or a deep link chose the category — so
 * onboarding, dialogs and errors can point at `?section=streaming` instead of
 * dumping the user at the top of a thirteen-section scroll.
 */
export function SettingsScreen() {
  const { user } = useAuth();
  const location = useLocation();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const desktop = useDesktopSettings();

  const native = isNative();
  const sections = useMemo(
    () => availableSettingsSections({ desktop: installsAvailable(), native }),
    [native],
  );
  const requested = resolveSettingsSection(params.get("section"), location.hash);
  const section = sections.find((entry) => entry.id === requested) ?? sections[0];
  const [focusToken, setFocusToken] = useState(() =>
    params.get("section") || location.hash ? 1 : 0,
  );

  // Normalise the address bar: a legacy `#help`, an unknown category or a
  // category this surface cannot show all land on the section actually drawn.
  useEffect(() => {
    if (params.get("section") !== section.id) {
      navigate(settingsSectionPath(section.id), { replace: true });
    }
  }, [location.key, navigate, params, section.id]);

  const selectSection = useCallback(() => setFocusToken((token) => token + 1), []);

  const body = (id: SettingsSectionId) => {
    switch (id) {
      case "account":
        return <AccountBody />;
      case "appearance":
        return <AppearanceBody desktop={desktop} />;
      case "language":
        return (
          <div className="border-t border-line pt-5">
            <LanguagePanel />
          </div>
        );
      case "storage":
        return <StorageBody desktop={desktop} />;
      case "downloads":
        return <DownloadsBody desktop={desktop} />;
      case "saves":
        return <SavesBody desktop={desktop} />;
      case "streaming":
        return <StreamingBody desktop={desktop} />;
      case "compatibility":
        return <CompatibilityBody desktop={desktop} />;
      case "integrations":
        return <IntegrationsBody desktop={desktop} />;
      case "profiles":
        return <ProfilesBody desktop={desktop} />;
      case "updates":
        return <UpdatesBody desktop={desktop} />;
      case "help":
        return <HelpBody />;
    }
  };

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Settings")}
        meta={
          user ? (
            <span className="num">
              {t("Signed in as")} {user.username}
            </span>
          ) : undefined
        }
        description={t("Everything that applies to this account, this browser and this device.")}
      />

      <div className="mx-auto mt-8 flex w-full max-w-[1240px] flex-col gap-8 lg:flex-row lg:items-start lg:gap-12">
        <SettingsNav sections={sections} active={section.id} onSelect={selectSection} />
        <div className="min-w-0 flex-1">
          <SettingsSection
            id={section.id}
            title={settingsSectionLabel(section.id)}
            description={settingsSectionDescription(section.id)}
            heading={section.id === "language" || section.id === "compatibility" ? "panel" : "section"}
            focusToken={focusToken}
          >
            {body(section.id)}
          </SettingsSection>
        </div>
      </div>
    </div>
  );
}
