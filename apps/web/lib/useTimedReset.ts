import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Returns [value, flash] where flash(next) sets the value and arms a timer to
 * reset back to idleValue after resetMs milliseconds. Any pending timer is
 * cleared before re-arming so rapid calls cannot cause an early revert. The
 * pending timer is also cleared on unmount.
 */
export function useTimedReset<T>(idleValue: T, resetMs: number): [T, (next: T) => void] {
  const [value, setValue] = useState<T>(idleValue);
  const timerRef = useRef<number | undefined>(undefined);

  const flash = useCallback(
    (next: T) => {
      if (timerRef.current !== undefined) {
        window.clearTimeout(timerRef.current);
      }
      setValue(next);
      timerRef.current = window.setTimeout(() => {
        timerRef.current = undefined;
        setValue(idleValue);
      }, resetMs);
    },
    [idleValue, resetMs],
  );

  // Clear any pending timer on unmount.
  useEffect(() => {
    return () => {
      if (timerRef.current !== undefined) {
        window.clearTimeout(timerRef.current);
      }
    };
  }, []);

  return [value, flash];
}
