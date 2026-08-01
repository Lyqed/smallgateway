type BrushFieldProps = {
  className?: string;
};

/**
 * Brush field — escalated (MURAL-DIRECTION). Still the human layer led by
 * violet, but now painterly and full-color: violet core with gold and
 * monarch bleeding off it, a crayon-worked edge, saturated enough to read
 * as paint on the machined wall. Sits behind content, bleeds across the
 * section edge. Never symmetric, never wallpaper. Inline SVG only.
 */
export function BrushField({ className }: BrushFieldProps) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 640 760"
      fill="none"
      className={className}
      preserveAspectRatio="xMinYMid slice"
    >
      <defs>
        <radialGradient id="bf-core" cx="30%" cy="42%" r="70%">
          <stop offset="0%" stopColor="var(--violet)" stopOpacity="0.78" />
          <stop offset="70%" stopColor="var(--violet)" stopOpacity="0.5" />
          <stop offset="100%" stopColor="var(--violet)" stopOpacity="0" />
        </radialGradient>
        <linearGradient id="bf-warm" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="var(--gold)" stopOpacity="0.75" />
          <stop offset="100%" stopColor="var(--monarch)" stopOpacity="0.45" />
        </linearGradient>
        <filter id="bf-f" x="-25%" y="-15%" width="150%" height="130%">
          <feTurbulence
            type="fractalNoise"
            baseFrequency="0.014 0.02"
            numOctaves="2"
            seed="8"
            result="n"
          />
          <feDisplacementMap
            in="SourceGraphic"
            in2="n"
            scale="46"
            xChannelSelector="R"
            yChannelSelector="G"
          />
        </filter>
      </defs>
      <g filter="url(#bf-f)">
        {/* widest violet wash */}
        <path
          d="M-100 120 C 80 40, 300 80, 360 200 C 420 316, 340 400, 430 480 C 512 552, 470 660, 340 700 C 200 744, 20 716, -80 640 Z"
          fill="url(#bf-core)"
        />
        {/* warm gold/monarch bleed, offset off the core */}
        <path
          d="M180 200 C 300 160, 400 210, 400 320 C 400 420, 300 470, 210 440 C 140 416, 120 320, 150 250 C 160 226, 168 210, 180 200 Z"
          fill="url(#bf-warm)"
        />
        {/* a teal flick escaping the field */}
        <path
          d="M300 156 C 372 128, 452 128, 500 152 C 452 156, 392 168, 340 190 C 322 180, 310 168, 300 156 Z"
          fill="var(--teal)"
          opacity="0.4"
        />
      </g>
    </svg>
  );
}
