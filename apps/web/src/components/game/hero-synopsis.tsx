import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";

/** Character threshold used as a layout-independent hint for long synopses. */
const LONG_SYNOPSIS_CHARS = 220;

/**
 * Clamped game synopsis for the hero header (#353). Keeps primary Play /
 * Install actions above the fold while letting readers expand the full summary
 * on demand.
 */
export function HeroSynopsis({
  description,
  className,
}: {
  description: string;
  className?: string;
}) {
  const ref = useRef<HTMLParagraphElement | null>(null);
  const [state, setState] = useState<{ description: string; expanded: boolean }>({
    description,
    expanded: false,
  });
  const [overflows, setOverflows] = useState(() => description.length > LONG_SYNOPSIS_CHARS);
  const expanded = state.description === description ? state.expanded : false;

  useEffect(() => {
    const el = ref.current;
    const longText = description.length > LONG_SYNOPSIS_CHARS;
    if (!el || expanded) {
      setOverflows((prev) => prev || longText);
      return;
    }
    const measure = () => {
      if (el.clientHeight > 0) {
        setOverflows(el.scrollHeight > el.clientHeight + 2);
      } else {
        setOverflows(longText);
      }
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [description, expanded]);

  return (
    <div className={cn("measure mt-5", className)}>
      <p
        ref={ref}
        className={cn(
          "text-lead leading-relaxed text-ink-2",
          !expanded && "line-clamp-3",
        )}
      >
        {description}
      </p>
      {overflows && (
        <button
          type="button"
          aria-expanded={expanded}
          onClick={() => setState({ description, expanded: !expanded })}
          className="mt-2 inline-flex items-center gap-1 text-label font-medium text-ink-3 transition-colors hover:text-ink"
        >
          {expanded ? (
            <>
              {t("Show less")}
              <ChevronUp className="size-3.5" aria-hidden="true" />
            </>
          ) : (
            <>
              {t("Read more")}
              <ChevronDown className="size-3.5" aria-hidden="true" />
            </>
          )}
        </button>
      )}
    </div>
  );
}
