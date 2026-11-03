/** Registers the "Add to collection" toolbar action. The page is routed in App. */
import { lazy } from "react";
import { registerAction } from "../../actions";

registerAction({
  id: "collections.add",
  label: "Add to collection",
  priority: 5,
  enabled: ({ selected }) => selected.length > 0,
  dialog: lazy(() => import("./AddToCollection")),
});
