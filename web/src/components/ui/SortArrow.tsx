import { cn } from "@/lib/cn";

/** Sort-direction indicator: solid arrow when active, faint glyph otherwise. */
export function SortArrow({ dir }: { dir: false | "asc" | "desc" }) {
  return (
    <span
      aria-hidden
      className={cn(
        "shrink-0 text-[10px] leading-none",
        dir ? "text-fg" : "text-muted opacity-40",
      )}
    >
      {dir === "asc" ? "↑" : dir === "desc" ? "↓" : "↕"}
    </span>
  );
}
