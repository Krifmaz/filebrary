/** Client for `/api/collections` and collection share links. */
import { request, type Share } from "../../api";

/** A collection in a listing. */
export interface CollectionSummary {
  id: number;
  name: string;
  item_count: number;
  created_at: number;
  updated_at: number;
}

/** One item of a collection. */
export interface CollectionItem {
  id: number;
  source: string;
  /** Path as the owner sees it; null when it left their scope. */
  path: string | null;
  name: string;
  /** null when the item is missing or out of reach. */
  kind: "file" | "dir" | null;
  size: number | null;
  mime: string | null;
}

/** A collection with its items. */
export interface Collection {
  id: number;
  name: string;
  items: CollectionItem[];
}

/** A file or folder to add. */
export interface NewItem {
  source: string;
  path: string;
}

/** Options for a share link to a collection. */
export interface CollectionShareInput {
  collection_id: number;
  password?: string;
  expires_at?: string | null;
  max_downloads?: number | null;
}

const base = "/api/collections";

export const collectionsApi = {
  list: () => request<CollectionSummary[]>("GET", base),
  get: (id: number) => request<Collection>("GET", `${base}/${id}`),
  create: (name: string, items: NewItem[] = []) => request<Collection>("POST", base, { name, items }),
  rename: (id: number, name: string) => request<void>("PATCH", `${base}/${id}`, { name }),
  remove: (id: number) => request<void>("DELETE", `${base}/${id}`),
  addItems: (id: number, items: NewItem[]) => request<Collection>("POST", `${base}/${id}/items`, { items }),
  removeItem: (id: number, item: number) => request<void>("DELETE", `${base}/${id}/items/${item}`),
  /** Creates a share link serving every item of the collection. */
  share: (input: CollectionShareInput) => request<Share>("POST", "/api/shares", input),
};
