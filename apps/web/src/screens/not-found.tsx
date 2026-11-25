import { Link } from "react-router-dom";
import { Button } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

export function NotFoundScreen() {
  return (
    <div className="page page-in flex min-h-[70vh] flex-col justify-center">
      <div className="max-w-[54ch] border-t border-line pt-8">
        <p className="flex items-center gap-2.5 text-micro font-semibold uppercase tracking-[0.14em] text-ink-3">
          <span className="h-[2px] w-7 bg-signal" aria-hidden="true" />
          {t("Error 404")}
        </p>
        <h1 className="mt-4 max-w-[20ch] text-display sm:text-hero font-semibold leading-[0.95] tracking-[-0.035em]">
          {t("This shelf is empty.")}
        </h1>
        <p className="mt-4 max-w-[46ch] text-lead leading-relaxed text-ink-2">
          {t("The page you asked for is not in the catalogue. It may have been moved, or the link may be older than this server.")}
        </p>
        <div className="mt-7 flex flex-wrap gap-2">
          <Link to="/library">
            <Button variant="primary">{t("Back to the library")}</Button>
          </Link>
          <Link to="/all">
            <Button variant="default">{t("Open the index")}</Button>
          </Link>
        </div>
      </div>
    </div>
  );
}
