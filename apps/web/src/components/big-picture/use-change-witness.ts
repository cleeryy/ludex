import { useLayoutEffect, useRef } from "react";

/* --------------------------------------------------------------------------
   A "did this value change while I was mounted?" witness (#220).

   A keyed Motion element replays its `initial` frame on every remount, which is
   right for a shelf change and wrong when the whole layout remounts (returning
   from Settings, a language change, a layout switch). The layout component
   survives a shelf change but not a remount, so its ref is exactly the
   witness we need: true only for the render that follows a real data change.
   -------------------------------------------------------------------------- */

export function useChangeWitness<T>(value: T): boolean {
  const seen = useRef(value);
  const changed = seen.current !== value;
  useLayoutEffect(() => {
    seen.current = value;
  }, [value]);
  return changed;
}
