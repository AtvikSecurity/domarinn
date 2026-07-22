// A compact chip showing a linked SSO identity: the provider icon + a display
// label, with the protocol/subject in a tooltip. Used on the admin user rows
// and the Settings account card.

import type { UserIdentityView } from "@/api";
import { ProviderIcon } from "@/components/icons/ProviderIcon";
import { Tooltip } from "@/components/ui/Tooltip";

/** "oidc:google" -> "google". */
function providerLabel(provider: string): string {
  const name = provider.includes(":")
    ? provider.slice(provider.indexOf(":") + 1)
    : provider;
  return name.charAt(0).toUpperCase() + name.slice(1);
}

export function ProviderBadge({ identity }: { identity: UserIdentityView }) {
  const label = providerLabel(identity.provider);
  return (
    <Tooltip content={`${identity.kind.toUpperCase()} · ${identity.subject}`}>
      <span
        tabIndex={0}
        className="inline-flex items-center gap-1 rounded-full bg-surface-2 px-1.5 py-0.5 text-[10px] font-medium text-muted outline-none ring-1 ring-inset ring-border focus-visible:ring-2 focus-visible:ring-ring"
      >
        <ProviderIcon
          provider={{ name: identity.provider, label, kind: identity.kind }}
          className="h-3 w-3"
        />
        {label}
      </span>
    </Tooltip>
  );
}
