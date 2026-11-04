/**
 * Results of the global search box: the indexed search across every source
 * the user can see, with type filters.
 */
import { useEffect, useState } from "react";
import type { Source } from "../api";
import { library, type SearchResults } from "../library";
import { formatDate, formatSize, parent } from "../paths";
import { filesUrl, navigate } from "../router";
import { EmptyState, ErrorState, Icon, PageHeader, SkeletonRows, fileIcon } from "../ui";

const TYPES: [string, string][] = [
  ["", "Everything"],
  ["dir", "Folders"],
  ["document", "Documents"],
  ["image", "Images"],
  ["video", "Videos"],
  ["audio", "Audio"],
  ["archive", "Archives"],
];

/** Props for {@link SearchPage}. */
export interface SearchPageProps {
  q: string;
  type: string;
  sources: Source[] | null;
}

const url = (q: string, type: string) => `/search?q=${encodeURIComponent(q)}${type ? `&type=${encodeURIComponent(type)}` : ""}`;

/** The search results page. */
export function SearchPage({ q, type, sources }: SearchPageProps) {
  const [retry, setRetry] = useState(0);
  const key = `${q}\u0000${type}\u0000${retry}`;
  // The outcome is tagged with the query it answers, so a new query shows the loading state at once.
  const [outcome, setOutcome] = useState<{ key: string; res?: SearchResults; error?: unknown } | null>(null);

  useEffect(() => {
    if (!q.trim()) return;
    const ctl = new AbortController();
    library.search({ q, type: type || undefined, limit: 100 }, ctl.signal).then(
      (res) => setOutcome({ key, res }),
      (error: unknown) => {
        if (!ctl.signal.aborted) setOutcome({ key, error: error ?? "Search failed" });
      },
    );
    return () => ctl.abort();
  }, [q, type, key]);

  const current = outcome?.key === key ? outcome : null;
  const res = current?.res ?? null;
  const error = current?.error ?? null;

  const name = (id: string) => sources?.find((s) => s.id === id)?.name ?? id;

  return (
    <div className="page">
      <PageHeader title={q ? `Results for “${q}”` : "Search"} description={res ? `${res.hits.length}${res.has_more ? "+" : ""} found${res.indexing ? " · still indexing, results may be incomplete" : ""}` : undefined} />
      <div className="chips" role="radiogroup" aria-label="Type">
        {TYPES.map(([t, label]) => (
          <button key={t} type="button" role="radio" aria-checked={type === t} className={type === t ? "chip active" : "chip"} onClick={() => navigate(url(q, t), true)}>
            {label}
          </button>
        ))}
      </div>
      {!q.trim() ? (
        <EmptyState icon="search" title="Search all your files">Type in the search box at the top to look across every source.</EmptyState>
      ) : error ? (
        <ErrorState title="Search failed" error={error} onRetry={() => setRetry((n) => n + 1)} />
      ) : res === null ? (
        <div className="card"><SkeletonRows rows={6} /></div>
      ) : res.hits.length === 0 ? (
        <div className="card">
          <EmptyState icon="search" title="No matches">Try fewer words, or another type.</EmptyState>
        </div>
      ) : (
        <ul className="card quick-list">
          {res.hits.map((h) => {
            const href = filesUrl(h.source, h.is_dir ? h.path : parent(h.path));
            return (
              <li key={`${h.source}:${h.path}`}>
                <a className="quick-item" href={href} onClick={(e) => { if (e.metaKey || e.ctrlKey) return; e.preventDefault(); navigate(href); }}>
                  <span className="quick-icon"><Icon name={fileIcon(h.name, h.is_dir)} /></span>
                  <span className="quick-text">
                    <span className="quick-name">{h.name}</span>
                    <span className="quick-path">{name(h.source)} · {h.path}</span>
                  </span>
                  <span className="quick-meta hide-phone">
                    {!h.is_dir && formatSize(h.size)}
                    {h.modified && <span>{formatDate(h.modified)}</span>}
                  </span>
                  <Icon name="chevronRight" className="quick-go" />
                </a>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
