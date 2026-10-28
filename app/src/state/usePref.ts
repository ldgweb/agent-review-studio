import { useCallback, useState } from 'react';

/** A view preference kept in local storage: read once, written on change.
 * Storage that is unavailable or holds an invalid value gives the fallback. */
export function usePref<T extends string | number | boolean>(
  key: string,
  fallback: T,
  valid: (raw: string) => boolean,
): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(key);
      if (raw !== null && valid(raw)) {
        return (typeof fallback === 'number' ? Number(raw) : typeof fallback === 'boolean' ? raw === 'true' : raw) as T;
      }
    } catch {
      // storage unavailable: the fallback
    }
    return fallback;
  });
  const set = useCallback(
    (next: T) => {
      setValue(next);
      try {
        localStorage.setItem(key, String(next));
      } catch {
        // a preference that does not stick is fine
      }
    },
    [key],
  );
  return [value, set];
}
