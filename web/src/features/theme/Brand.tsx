/** The product logo and name from the branding settings. */
import { useBranding } from "./branding";

/** Logo plus name, as used in the header, the login page and share pages. */
export function BrandMark({ size = 28 }: { size?: number }) {
  const b = useBranding();
  return (
    <>
      <img src={b.logo_url ?? "/icon.svg"} alt="" height={size} />
      <span>{b.name}</span>
    </>
  );
}
