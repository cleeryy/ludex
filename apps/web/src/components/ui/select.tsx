import { useCallback, useRef, useState, type ReactNode } from "react";
import { Check, ChevronDown } from "lucide-react";
import { cn } from "@/lib/cn";
import { Popover, MenuItem, MenuSeparator, MenuLabel } from "@/components/ui/popover";

/* ------------------------------- dropdown -------------------------------- */

export interface MenuAction {
  label: string;
  onSelect: () => void;
  icon?: ReactNode;
  kbd?: string;
  danger?: boolean;
  disabled?: boolean;
  selected?: boolean;
}

export type MenuEntry = MenuAction | "separator" | { heading: string };

export function useDropdown() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  const toggle = useCallback(() => setOpen((v) => !v), []);
  return {
    open,
    setOpen,
    ref,
    toggle,
    triggerProps: {
      ref,
      onClick: toggle,
      "aria-haspopup": "menu" as const,
      "aria-expanded": open,
    },
    menuProps: { open, onOpenChange: setOpen, anchorRef: ref },
  };
}

export function DropdownMenu({
  items,
  open,
  onOpenChange,
  anchorRef,
  align = "end",
  label,
}: {
  items: MenuEntry[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  anchorRef: React.RefObject<HTMLButtonElement | null>;
  align?: "start" | "end";
  label?: string;
}) {
  return (
    <Popover
      open={open}
      onOpenChange={onOpenChange}
      anchorRef={anchorRef}
      align={align}
      role="menu"
      label={label}
      initialFocus="first"
      className="min-w-[220px] py-1"
    >
      {items.map((item, index) => {
        if (item === "separator") return <MenuSeparator key={`sep-${index}`} />;
        if ("heading" in item) return <MenuLabel key={`head-${index}`}>{item.heading}</MenuLabel>;
        return (
          <MenuItem
            key={item.label}
            onSelect={() => {
              onOpenChange(false);
              item.onSelect();
            }}
            icon={item.icon}
            kbd={item.kbd}
            danger={item.danger}
            disabled={item.disabled}
            selected={item.selected}
          >
            {item.label}
          </MenuItem>
        );
      })}
    </Popover>
  );
}

/* --------------------------------- select -------------------------------- */

export interface SelectOption<T extends string | number> {
  value: T;
  label: string;
  hint?: string;
}

export function Select<T extends string | number>({
  value,
  onChange,
  options,
  label,
  id,
  placeholder = "Select",
  className,
  disabled,
  align = "start",
}: {
  value: T | null;
  onChange: (value: T) => void;
  options: SelectOption<T>[];
  label: string;
  id?: string;
  placeholder?: string;
  className?: string;
  disabled?: boolean;
  align?: "start" | "end";
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  const selected = options.find((option) => option.value === value);

  return (
    <>
      <button
        id={id}
        ref={ref}
        type="button"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={label}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            setOpen(true);
          }
        }}
        className={cn(
          "flex h-11 w-full items-center justify-between gap-2 rounded-[2px] border border-line bg-surface px-3 text-left text-[14px] text-ink",
          "transition-colors duration-[120ms] hover:border-line-2 disabled:opacity-40",
          !selected && "text-ink-3",
          className,
        )}
      >
        <span className="truncate">{selected?.label ?? placeholder}</span>
        <ChevronDown className={cn("size-4 shrink-0 text-ink-3 transition-transform duration-150", open && "rotate-180")} aria-hidden="true" />
      </button>
      <Popover
        open={open}
        onOpenChange={setOpen}
        anchorRef={ref}
        align={align}
        matchWidth
        role="listbox"
        label={label}
        initialFocus="first"
        className="py-1"
      >
        {options.map((option) => (
          <button
            key={String(option.value)}
            type="button"
            role="option"
            data-menu-item
            aria-selected={option.value === value}
            onClick={() => {
              onChange(option.value);
              setOpen(false);
              ref.current?.focus();
            }}
            className={cn(
              "flex h-9 w-full items-center gap-2 px-3 text-left text-[13.5px] transition-colors duration-[100ms]",
              "text-ink-2 hover:bg-surface-2 hover:text-ink",
              option.value === value && "text-ink",
            )}
          >
            <span className="flex-1 truncate">{option.label}</span>
            {option.hint && <span className="num text-[11px] text-ink-3">{option.hint}</span>}
            {option.value === value && <Check className="size-3.5 text-signal-text" aria-hidden="true" />}
          </button>
        ))}
      </Popover>
    </>
  );
}
