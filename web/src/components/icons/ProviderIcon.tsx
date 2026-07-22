// Inline SVG glyphs for SSO provider buttons — no external icon dependency.
// Brand marks are drawn from the provider's public logo geometry; unknown
// providers fall back to a generic key (OIDC) or shield (SAML) so every
// button still gets an icon.

import type { SsoProviderMeta } from "@/api";

type IconProps = { className?: string };

/** The multi-color Google "G". */
function GoogleIcon({ className }: IconProps) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden>
      <path
        fill="#4285F4"
        d="M23.52 12.27c0-.82-.07-1.6-.21-2.36H12v4.48h6.47a5.53 5.53 0 0 1-2.4 3.63v3h3.88c2.27-2.09 3.57-5.17 3.57-8.75Z"
      />
      <path
        fill="#34A853"
        d="M12 24c3.24 0 5.96-1.08 7.95-2.91l-3.88-3c-1.08.72-2.45 1.16-4.07 1.16-3.13 0-5.78-2.11-6.73-4.96H1.29v3.09A12 12 0 0 0 12 24Z"
      />
      <path
        fill="#FBBC05"
        d="M5.27 14.29a7.2 7.2 0 0 1 0-4.58V6.62H1.29a12 12 0 0 0 0 10.76l3.98-3.09Z"
      />
      <path
        fill="#EA4335"
        d="M12 4.75c1.77 0 3.35.61 4.6 1.8l3.44-3.44A11.95 11.95 0 0 0 12 0 12 12 0 0 0 1.29 6.62l3.98 3.09C6.22 6.86 8.87 4.75 12 4.75Z"
      />
    </svg>
  );
}

/** A single-path glyph tinted with the current text color. */
function PathIcon({ className, d }: IconProps & { d: string }) {
  return (
    <svg viewBox="0 0 24 24" className={className} fill="currentColor" aria-hidden>
      <path d={d} />
    </svg>
  );
}

const MICROSOFT_D =
  "M11.4 3H3v8.4h8.4V3Zm9.6 0h-8.4v8.4H21V3Zm-9.6 9.6H3V21h8.4v-8.4Zm9.6 0h-8.4V21H21v-8.4Z";
const GITLAB_D =
  "m12 21 3.7-11.4H8.3L12 21ZM3 9.6 4.3 5.5c.1-.2.4-.2.5 0l1.4 4.1H3Zm18 0h-3.2l1.4-4.1c.1-.2.4-.2.5 0L21 9.6ZM12 21 3 9.6h5.3L12 21Zm0 0 9-11.4h-5.3L12 21Z";
const KEY_D =
  "M14 2a6 6 0 0 0-5.7 7.9L2 16.2V22h5.8l.6-.6v-2h2v-2h2l1.3-1.3A6 6 0 1 0 14 2Zm2.5 5.5a1.5 1.5 0 1 1 0-3 1.5 1.5 0 0 1 0 3Z";
const SHIELD_D =
  "M12 2 4 5v6c0 4.4 3.4 8.6 8 10 4.6-1.4 8-5.6 8-10V5l-8-3Zm0 5.5a2.5 2.5 0 0 1 2.5 2.5c0 1-.6 1.9-1.5 2.3V15h-2v-2.7A2.5 2.5 0 0 1 9.5 10 2.5 2.5 0 0 1 12 7.5Z";
// A simplified Authentik/Okta-style "ring" mark.
const RING_D =
  "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20Zm0 4a6 6 0 1 1 0 12 6 6 0 0 1 0-12Zm0 3a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z";

/** Pick a glyph for the provider by matching its id/label, then by kind. */
export function ProviderIcon({
  provider,
  className = "h-4 w-4",
}: {
  provider: Pick<SsoProviderMeta, "name" | "label" | "kind">;
  className?: string;
}) {
  const key = `${provider.name} ${provider.label}`.toLowerCase();
  if (key.includes("google")) return <GoogleIcon className={className} />;
  if (
    key.includes("microsoft") ||
    key.includes("entra") ||
    key.includes("azure")
  )
    return <PathIcon className={className} d={MICROSOFT_D} />;
  if (key.includes("gitlab")) return <PathIcon className={className} d={GITLAB_D} />;
  if (key.includes("okta") || key.includes("authentik") || key.includes("keycloak"))
    return <PathIcon className={className} d={RING_D} />;
  // Fallbacks by protocol.
  return provider.kind === "saml" ? (
    <PathIcon className={className} d={SHIELD_D} />
  ) : (
    <PathIcon className={className} d={KEY_D} />
  );
}
