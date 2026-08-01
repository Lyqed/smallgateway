import type { AnchorHTMLAttributes, ReactNode } from "react";

type ButtonLinkProps = {
  href: string;
  variant?: "monarch" | "outline";
  children: ReactNode;
} & AnchorHTMLAttributes<HTMLAnchorElement>;

const BASE =
  "inline-flex items-center justify-center gap-2 rounded-sm px-5 py-2.5 text-sm font-medium tracking-tight transition-[transform,box-shadow,background-color] duration-150";

const VARIANTS: Record<NonNullable<ButtonLinkProps["variant"]>, string> = {
  // Monarch = primary action (semantic role, brief §2). Ink-dark text
  // on monarch clears AA; hover lifts with a hard sunlight shadow.
  monarch:
    "bg-monarch text-floor hover:-translate-y-0.5 hover:shadow-[4px_5px_0_0_oklch(from_var(--steel)_l_c_h_/_0.55)] focus-visible:outline-ink",
  outline:
    "border-2 border-ink text-ink hover:-translate-y-0.5 hover:bg-panel hover:shadow-[4px_5px_0_0_oklch(from_var(--steel)_l_c_h_/_0.45)]",
};

export function ButtonLink({
  href,
  variant = "monarch",
  children,
  ...rest
}: ButtonLinkProps) {
  return (
    <a href={href} className={`${BASE} ${VARIANTS[variant]}`} {...rest}>
      {children}
    </a>
  );
}
