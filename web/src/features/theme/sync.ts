/**
 * Keeps the theme in step with the server: after sign-in the stored theme
 * and custom colours are applied, and later changes are saved back.
 */
import { onThemeChange, setTheme } from "../../theme";
import { getUserColours, setUserColours, themeApi } from "./branding";

let signedIn = false;

onThemeChange((theme) => {
  if (signedIn) void themeApi.put({ theme, colours: getUserColours() }).catch(() => {});
});

/** Loads the user's theme after sign-in (call with `false` on sign-out). */
export async function syncUserTheme(isSignedIn: boolean): Promise<void> {
  signedIn = isSignedIn;
  if (!isSignedIn) return;
  try {
    const t = await themeApi.get();
    setUserColours(t.colours);
    setTheme(t.theme, true);
  } catch {
    /* keep the local choice */
  }
}
