import { forwardRef, useId } from "react";
import type { InputHTMLAttributes, ReactNode } from "react";
import { cn } from "@/lib/cn";

export interface TextFieldProps
  extends InputHTMLAttributes<HTMLInputElement> {
  label: string;
  hint?: ReactNode;
}

/** Labelled text input used across the auth + account forms. */
export const TextField = forwardRef<HTMLInputElement, TextFieldProps>(
  ({ label, hint, id, className, ...props }, ref) => {
    const autoId = useId();
    const inputId = id ?? autoId;
    return (
      <div className="flex flex-col gap-1">
        <label htmlFor={inputId} className="text-sm font-medium text-fg">
          {label}
        </label>
        <input
          ref={ref}
          id={inputId}
          className={cn(
            "h-9 w-full rounded-md border border-border bg-bg px-3 text-sm text-fg outline-none focus:ring-2 focus:ring-ring disabled:opacity-60",
            className,
          )}
          {...props}
        />
        {hint ? <span className="text-xs text-muted">{hint}</span> : null}
      </div>
    );
  },
);
TextField.displayName = "TextField";
