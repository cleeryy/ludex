import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/cn";

type Align = "start" | "end" | "center";

export interface PopoverProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  anchorRef: RefObject<HTMLElement | null>;
  children: ReactNode;
  align?: Align;
  side?: "bottom" | "top";
  matchWidth?: boolean;
  className?: string;
  role?: string;
  label?: string;
  initialFocus?: "first" | "container" | "none";
  offset?: number;
  maxHeight?: number;
}

/**
 * Portalled floating panel. Fixed positioning, viewport clamping, Escape,
 * outside-click, focus restore. No dependency, no transform tricks that break
 * inside scrolled containers.
 */
export function Popover({
  open,
  onOpenChange,
  anchorRef,
  children,
  align = "start",
  side = "bottom",
  matchWidth = false,
  className,
  role = "dialog",
  label,
  initialFocus = "container",
  offset = 6,
  maxHeight = 360,
}: PopoverProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const [style, setStyle] = useState<CSSProperties>({ opacity: 0 });
  const [placed, setPlaced] = useState(false);

  const place = useCallback(() => {
    const anchor = anchorRef.current;
    if (!anchor) return;
    const rect = anchor.getBoundingClientRect();
    const margin = 8;
    const panel = panelRef.current;
    const width = matchWidth ? rect.width : (panel?.offsetWidth ?? 240);
    const height = Math.min(panel?.offsetHeight ?? 240, maxHeight);

    let left = rect.left;
    if (align === "end") left = rect.right - width;
    if (align === "center") left = rect.left + rect.width / 2 - width / 2;
    left = Math.max(margin, Math.min(left, window.innerWidth - width - margin));

    let top = side === "bottom" ? rect.bottom + offset : rect.top - height - offset;
    if (side === "bottom" && top + height > window.innerHeight - margin) {
      top = Math.max(margin, rect.top - height - offset);
    }
    if (side === "top" && top < margin) {
      top = Math.min(window.innerHeight - height - margin, rect.bottom + offset);
    }

    setStyle({
      position: "fixed",
      top,
      left,
      width: matchWidth ? rect.width : undefined,
      minWidth: matchWidth ? undefined : 180,
      maxHeight,
      zIndex: 90,
    });
    setPlaced(true);
  }, [align, anchorRef, matchWidth, maxHeight, offset, side]);

  useLayoutEffect(() => {
    if (!open) {
      setPlaced(false);
      return;
    }
    place();
  }, [open, place]);

  useEffect(() => {
    if (!open) return;
    const onScroll = () => place();
    const onResize = () => place();
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onResize);
    };
  }, [open, place]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (panelRef.current?.contains(target)) return;
      if (anchorRef.current?.contains(target)) return;
      onOpenChange(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onOpenChange(false);
        anchorRef.current?.focus();
      }
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  }, [open, onOpenChange, anchorRef]);

  useEffect(() => {
    if (!open) return;
    const panel = panelRef.current;
    if (!panel) return;
    if (initialFocus === "none") return;
    const target =
      initialFocus === "first"
        ? (panel.querySelector<HTMLElement>("[data-menu-item]:not([disabled])") ?? panel)
        : panel;
    requestAnimationFrame(() => target.focus({ preventScroll: true }));
  }, [open, initialFocus]);

  if (!open) return null;

  return createPortal(
    <div
      ref={panelRef}
      style={style}
      role={role}
      aria-label={label}
      tabIndex={-1}
      data-placed={placed}
      className={cn(
        "overflow-y-auto rounded-[2px] border border-line-2 bg-surface text-ink outline-none",
        "shadow-[0_18px_50px_-12px_rgba(0,0,0,0.55)]",
        "data-[placed=true]:animate-[pop-in_140ms_cubic-bezier(0.2,0.6,0.2,1)_both]",
        className,
      )}
    >
      {children}
    </div>,
    document.body,
  );
}

export function MenuItem({
  children,
  onSelect,
  icon,
  kbd,
  danger,
  disabled,
  selected,
}: {
  children: ReactNode;
  onSelect: () => void;
  icon?: ReactNode;
  kbd?: string;
  danger?: boolean;
  disabled?: boolean;
  selected?: boolean;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      data-menu-item
      disabled={disabled}
      aria-checked={selected}
      onClick={onSelect}
      className={cn(
        "flex h-9 w-full items-center gap-2.5 px-3 text-left text-[13.5px] transition-colors duration-[100ms]",
        "disabled:pointer-events-none disabled:opacity-40",
        danger ? "text-danger hover:bg-danger/10" : "text-ink-2 hover:bg-surface-2 hover:text-ink",
        selected && "text-ink",
      )}
    >
      {icon && <span className="grid size-4 place-items-center text-ink-3">{icon}</span>}
      <span className="flex-1 truncate">{children}</span>
      {selected && <span className="size-1.5 rounded-full bg-signal" aria-hidden="true" />}
      {kbd && <span className="kbd">{kbd}</span>}
    </button>
  );
}

export function MenuSeparator() {
  return <div className="my-1 h-px bg-line" role="separator" />;
}

export function MenuLabel({ children }: { children: ReactNode }) {
  return <div className="px-3 pb-1 pt-2 text-[11px] font-medium text-ink-3">{children}</div>;
}
