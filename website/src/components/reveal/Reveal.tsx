"use client";

import { useEffect, useRef, type ReactNode } from "react";

type RevealProps = {
  children: ReactNode;
  className?: string;
  /** Stagger delay in ms, applied once the element intersects. */
  delay?: number;
};

/**
 * The single client component on the site: an IntersectionObserver
 * reveal wrapper. The machined layer fades in; hand strokes inside it
 * (`.draw-path`) draw themselves — both driven purely by CSS classes.
 *
 * Without JS (or with prefers-reduced-motion) nothing is ever hidden:
 * the armed state is only applied after mount, and reduced-motion CSS
 * neutralizes it entirely.
 */
export function Reveal({ children, className, delay = 0 }: RevealProps) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;

    if (delay > 0) {
      el.style.setProperty("--reveal-delay", `${delay}ms`);
    }
    el.classList.add("reveal-armed");

    const observer = new IntersectionObserver(
      (entries, io) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            el.classList.add("is-revealed");
            io.disconnect();
          }
        }
      },
      { threshold: 0.15, rootMargin: "0px 0px -6% 0px" },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [delay]);

  return (
    <div ref={ref} className={className}>
      {children}
    </div>
  );
}
