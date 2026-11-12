/**
 * #192: one reusable confirmation for a privileged action.
 *
 * It shows exactly what will change, the target and the reason before Windows
 * asks for permission, offers "Continue as administrator" and a cancel path,
 * and keeps the failure message inside the dialog so the manual fallback stays
 * visible.
 */
import { ShieldCheck } from "lucide-react";
import { Dialog } from "@/components/ui/dialog";
import { Button } from "@/components/ui/controls";
import { elevationOperationLabel, type ElevationPrompt } from "@/lib/elevation";
import { t } from "@/lib/i18n";

function Row({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex flex-col gap-0.5 border-b border-line py-2">
      <dt className="text-[11px] uppercase tracking-[0.06em] text-ink-3">{label}</dt>
      <dd className={mono ? "break-all font-mono text-[11px] text-ink-2" : "text-[12px] text-ink-2"}>
        {value}
      </dd>
    </div>
  );
}

export function ElevationConfirm({
  open,
  prompt,
  busy = false,
  error = null,
  onOpenChange,
  onContinue,
}: {
  open: boolean;
  prompt: ElevationPrompt | null;
  /** True while the UAC prompt is open on the desktop side. */
  busy?: boolean;
  /** Failure returned after the prompt; the dialog stays open for a retry. */
  error?: string | null;
  onOpenChange: (open: boolean) => void;
  onContinue: () => void;
}) {
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!busy) onOpenChange(next);
      }}
      title={t("Administrator rights are needed")}
      description={t("Windows will ask for permission once. Only this operation runs as administrator; Ludex keeps running as your normal user, and cancelling changes nothing.")}
      size="sm"
      dismissible={!busy}
      footer={
        <>
          <Button variant="ghost" disabled={busy} onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button
            variant="primary"
            icon={<ShieldCheck className="size-3.5" />}
            loading={busy}
            onClick={onContinue}
          >
            {t("Continue as administrator")}
          </Button>
        </>
      }
    >
      {prompt && (
        <div className="flex flex-col gap-3">
          <dl>
            <Row label={t("What will change")} value={elevationOperationLabel(prompt.operation)} />
            <Row label={t("Target")} value={prompt.target} mono />
            <Row label={t("Why")} value={prompt.reason} />
          </dl>
          <p className="text-[11.5px] leading-relaxed text-ink-3">
            {t("If you cancel, nothing is changed and the manual instructions stay available.")}
          </p>
          {error && (
            <p role="alert" className="border-l-2 border-danger pl-3 text-[12px] leading-relaxed text-danger">
              {error}
            </p>
          )}
        </div>
      )}
    </Dialog>
  );
}
