/** Every file and folder the user starred, newest first, with unstar. */
import { useCallback, useEffect, useState } from "react";
import type { Source } from "../api";
import { library, type Star } from "../library";
import { EmptyState, ErrorState, IconButton, PageHeader, SkeletonRows, toast } from "../ui";
import { StarLink } from "./HomePage";

/** The starred page. */
export function StarredPage({ sources }: { sources: Source[] | null }) {
  const [stars, setStars] = useState<Star[] | null>(null);
  const [error, setError] = useState<unknown>(null);

  const fetchStars = useCallback(() => {
    library.stars().then(
      (list) => {
        setStars(list);
        setError(null);
      },
      (e: unknown) => setError(e),
    );
  }, []);
  useEffect(fetchStars, [fetchStars]);
  const load = () => {
    setError(null);
    setStars(null);
    fetchStars();
  };

  const unstar = async (st: Star) => {
    try {
      await library.unstar(st.source, st.path);
      setStars((list) => list?.filter((x) => x !== st) ?? null);
      toast(`Removed ${st.name} from Starred`, { label: "Undo", run: () => void library.star(st.source, st.path).then(load) });
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="page">
      <PageHeader title="Starred" description="Files and folders you starred, newest first." />
      {error ? (
        <ErrorState error={error} onRetry={load} />
      ) : stars === null ? (
        <div className="card"><SkeletonRows rows={5} /></div>
      ) : stars.length === 0 ? (
        <div className="card">
          <EmptyState icon="star" title="Nothing starred yet">
            Right-click a file or folder and choose <strong>Star</strong> to find it here.
          </EmptyState>
        </div>
      ) : (
        <ul className="card quick-list">
          {stars.map((st) => (
            <li key={`${st.source}:${st.path}`} className="quick-row">
              <StarLink star={st} sources={sources} />
              <IconButton icon="star" filled label={`Unstar ${st.name}`} className="star-on" onClick={() => void unstar(st)} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
