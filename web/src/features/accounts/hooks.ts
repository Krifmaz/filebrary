/** Small React helpers shared by the account pages. */
import { useCallback, useEffect, useState } from "react";
import { errorMessage } from "../../ui";

/** The state of a request started by {@link useAsync}. */
export interface AsyncState<T> {
  data: T | undefined;
  error: string | null;
  loading: boolean;
  /** Runs the request again. */
  reload: () => void;
}

/**
 * Runs `fn` when the component mounts and whenever `deps` change, keeping
 * the last result, error and loading flag. Results of superseded runs are
 * ignored.
 */
export function useAsync<T>(fn: () => Promise<T>, deps: readonly unknown[]): AsyncState<T> {
  const [data, setData] = useState<T | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  const reload = useCallback(() => setTick((n) => n + 1), []);
  useEffect(() => {
    let live = true;
    // Mark as loading from the promise chain, not synchronously in the effect.
    Promise.resolve()
      .then(() => live && setLoading(true))
      .then(fn)
      .then((d) => {
        if (!live) return;
        setData(d);
        setError(null);
      })
      .catch((e: unknown) => live && setError(errorMessage(e)))
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  return { data, error, loading, reload };
}
