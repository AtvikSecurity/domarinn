import type { ComponentDrift } from "@/api";
import { changedComponents, componentLabel } from "@/lib/digests";
import { Chip } from "./ui/Chip";
import { Tooltip } from "./ui/Tooltip";

/**
 * Names the suite components that changed between two runs.
 *
 * Renders only what *did* change, so it has zero footprint in the common case
 * where nothing did. The alternative — a fixed strip of five glyphs, lit or
 * unlit — would be permanent visual noise serving a rare event, and would
 * invent a private glyph language where chips already exist.
 */
export function ChangeChips({
  drift,
  className,
}: {
  drift: ComponentDrift[];
  className?: string;
}) {
  const changed = changedComponents(drift);
  if (changed.length === 0) return null;
  return (
    <span className={className}>
      {changed.map((component) => (
        <Tooltip
          key={component}
          content={`The suite's ${componentLabel(component)} changed between these runs`}
        >
          <Chip tone="amber" size="xs" className="mr-1">
            {componentLabel(component)} changed
          </Chip>
        </Tooltip>
      ))}
    </span>
  );
}
