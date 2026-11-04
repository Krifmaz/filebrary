/** Registers the "Comments" toolbar action. */
import { lazy } from "react";
import { registerAction } from "../../actions";

registerAction({
  id: "comments",
  label: "Comments",
  icon: "comment",
  priority: 10,
  // One selected item, or nothing selected to comment on the folder itself.
  enabled: ({ selected, path }) => selected.length === 1 || (selected.length === 0 && path !== "/"),
  dialog: lazy(() => import("./CommentsDialog")),
});
