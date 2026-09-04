type AstronautProps = {
  className?: string;
};

/**
 * The heart image (MURAL-DIRECTION §1): an astronaut reaching toward a
 * single flower. Hand-drawn, painterly, crayon-quality — loose strokes,
 * wobbled outlines, monarch / violet / gold tones over a deep-blue
 * planet, color-splash arcs bleeding out of frame. Entirely inline SVG,
 * aria-hidden, themeable via the mural tokens. No external assets.
 *
 * Ids are namespaced with a required `id` prop because SVG gradient/filter
 * ids are document-global and this figure may appear more than once.
 */
export function Astronaut({ className }: AstronautProps) {
  const paint = "aq"; // gradient/id namespace
  return (
    <svg
      aria-hidden
      viewBox="0 0 520 620"
      fill="none"
      className={className}
      preserveAspectRatio="xMidYMid meet"
    >
      <defs>
        {/* deep-blue planet body */}
        <radialGradient id={`${paint}-planet`} cx="38%" cy="34%" r="80%">
          <stop offset="0%" stopColor="oklch(46% 0.16 265)" />
          <stop offset="55%" stopColor="oklch(34% 0.16 268)" />
          <stop offset="100%" stopColor="oklch(22% 0.12 270)" />
        </radialGradient>
        {/* the color-splash arc: gold → violet → teal */}
        <linearGradient id={`${paint}-arc`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="var(--gold)" />
          <stop offset="52%" stopColor="var(--violet)" />
          <stop offset="100%" stopColor="var(--teal)" />
        </linearGradient>
        {/* suit sheen */}
        <linearGradient id={`${paint}-suit`} x1="0" y1="0" x2="0.4" y2="1">
          <stop offset="0%" stopColor="oklch(99% 0.003 250)" />
          <stop offset="100%" stopColor="oklch(88% 0.01 255)" />
        </linearGradient>
        {/* crayon roughness — hand-drawn wobble on strokes/fills */}
        <filter id={`${paint}-crayon`} x="-15%" y="-15%" width="130%" height="130%">
          <feTurbulence
            type="fractalNoise"
            baseFrequency="0.9"
            numOctaves="1"
            seed="7"
            result="n"
          />
          <feDisplacementMap
            in="SourceGraphic"
            in2="n"
            scale="3.2"
            xChannelSelector="R"
            yChannelSelector="G"
          />
        </filter>
      </defs>

      {/* ---- color-splash arcs, bleeding off frame (behind everything) ---- */}
      <g opacity="0.9" filter={`url(#${paint}-crayon)`}>
        <path
          d="M-40 470 C 90 300, 250 210, 470 176 C 540 168, 560 170, 580 178"
          stroke={`url(#${paint}-arc)`}
          strokeWidth="42"
          strokeLinecap="round"
          opacity="0.5"
        />
        <path
          d="M-30 520 C 120 360, 300 268, 520 244"
          stroke="var(--monarch)"
          strokeWidth="16"
          strokeLinecap="round"
          opacity="0.55"
        />
        <path
          d="M40 596 C 150 470, 330 388, 540 372"
          stroke="var(--teal)"
          strokeWidth="10"
          strokeLinecap="round"
          opacity="0.5"
        />
      </g>

      {/* ---- the deep-blue planet, lower-left, cropped ---- */}
      <g filter={`url(#${paint}-crayon)`}>
        <circle cx="120" cy="560" r="180" fill={`url(#${paint}-planet)`} />
        {/* hand-scribbled surface bands */}
        <path
          d="M-20 540 C 60 528, 150 534, 250 552 M 0 590 C 90 580, 180 586, 268 604 M 30 500 C 100 492, 170 496, 236 508"
          stroke="oklch(58% 0.14 262)"
          strokeWidth="4"
          strokeLinecap="round"
          opacity="0.55"
          fill="none"
        />
        {/* a small crayon crater */}
        <circle cx="86" cy="520" r="16" fill="oklch(28% 0.12 270)" opacity="0.6" />
        <circle cx="170" cy="588" r="10" fill="oklch(28% 0.12 270)" opacity="0.5" />
      </g>

      {/* ---- the single flower, up-right, reached-for ---- */}
      <g filter={`url(#${paint}-crayon)`}>
        {/* stem */}
        <path
          d="M436 214 C 430 184, 434 150, 448 118"
          stroke="var(--teal-deep)"
          strokeWidth="6"
          strokeLinecap="round"
          fill="none"
        />
        {/* leaf */}
        <path
          d="M434 178 C 410 170, 392 178, 388 196 C 406 200, 428 194, 434 178 Z"
          fill="var(--teal)"
        />
        {/* petals — monarch + gold, loose crayon ovals */}
        <g>
          <ellipse cx="448" cy="86" rx="18" ry="30" fill="var(--gold)" transform="rotate(-18 448 86)" />
          <ellipse cx="486" cy="104" rx="18" ry="30" fill="var(--monarch)" transform="rotate(54 486 104)" />
          <ellipse cx="480" cy="150" rx="18" ry="30" fill="var(--gold)" transform="rotate(128 480 150)" />
          <ellipse cx="436" cy="152" rx="18" ry="30" fill="var(--monarch)" transform="rotate(-128 436 152)" />
          <ellipse cx="416" cy="108" rx="18" ry="30" fill="var(--gold)" transform="rotate(-56 416 108)" />
          {/* flower heart */}
          <circle cx="450" cy="118" r="18" fill="var(--violet)" />
          <circle cx="450" cy="118" r="9" fill="var(--gold)" />
        </g>
      </g>

      {/* ---- the astronaut, mid-body, reaching up-right toward the flower ---- */}
      <g
        stroke="var(--ink)"
        strokeWidth="4.5"
        strokeLinejoin="round"
        strokeLinecap="round"
        filter={`url(#${paint}-crayon)`}
      >
        {/* backpack */}
        <path
          d="M196 360 C 176 356, 168 384, 172 424 C 176 456, 196 470, 224 466 L 224 356 C 214 354, 204 356, 196 360 Z"
          fill="oklch(82% 0.012 255)"
        />
        {/* torso */}
        <path
          d="M220 340 C 262 330, 300 336, 314 366 C 324 392, 320 448, 300 486 C 276 494, 236 494, 214 484 C 200 446, 200 388, 208 356 C 212 348, 216 343, 220 340 Z"
          fill={`url(#${paint}-suit)`}
        />
        {/* chest control patch — a splash of mural color on the machined suit */}
        <rect x="240" y="392" width="46" height="34" rx="5" fill="var(--surface-panel)" />
        <circle cx="252" cy="404" r="5" fill="var(--teal)" stroke="none" />
        <circle cx="270" cy="404" r="5" fill="var(--gold)" stroke="none" />
        <circle cx="252" cy="418" r="5" fill="var(--violet)" stroke="none" />
        <circle cx="270" cy="418" r="5" fill="var(--monarch)" stroke="none" />
        {/* reaching (right) arm, up toward the flower */}
        <path
          d="M306 360 C 344 336, 380 292, 408 232 C 414 220, 400 210, 390 220 C 360 268, 330 306, 296 336 C 296 344, 300 353, 306 360 Z"
          fill={`url(#${paint}-suit)`}
        />
        {/* reaching glove — open hand, fingers toward the bloom */}
        <path
          d="M404 236 C 416 216, 434 200, 452 196 C 462 194, 466 204, 458 214 C 462 208, 470 206, 474 212 C 478 218, 472 226, 462 232 C 468 230, 474 234, 472 242 C 470 250, 458 252, 448 250 C 440 258, 424 260, 410 254 C 400 250, 398 242, 404 236 Z"
          fill={`url(#${paint}-suit)`}
        />
        {/* trailing (left) arm, tucked */}
        <path
          d="M212 372 C 186 388, 168 420, 168 456 C 168 468, 182 470, 190 462 C 196 430, 210 404, 226 386 C 224 380, 218 375, 212 372 Z"
          fill={`url(#${paint}-suit)`}
        />
        {/* legs — loose, floating */}
        <path
          d="M232 488 C 218 528, 208 566, 216 600 C 220 612, 234 610, 238 598 C 244 566, 256 530, 268 500 C 258 492, 244 488, 232 488 Z"
          fill={`url(#${paint}-suit)`}
        />
        <path
          d="M276 490 C 288 528, 304 558, 328 578 C 338 586, 348 576, 342 564 C 322 538, 308 512, 300 486 C 292 486, 284 488, 276 490 Z"
          fill={`url(#${paint}-suit)`}
        />
        {/* helmet */}
        <circle cx="252" cy="300" r="58" fill={`url(#${paint}-suit)`} />
        {/* visor — deep, catching a sliver of sky + a reflected flower */}
        <ellipse cx="256" cy="302" rx="40" ry="44" fill="oklch(28% 0.1 262)" stroke="none" />
        <ellipse cx="256" cy="302" rx="40" ry="44" fill="none" stroke="var(--ink)" strokeWidth="4.5" />
        {/* visor highlight — a crayon streak of skylight */}
        <path
          d="M238 276 C 250 268, 266 268, 278 276"
          stroke="var(--skylight)"
          strokeWidth="6"
          strokeLinecap="round"
          fill="none"
          opacity="0.85"
        />
        {/* a tiny reflected flower dot in the visor: monarch */}
        <circle cx="272" cy="316" r="6" fill="var(--monarch)" stroke="none" opacity="0.9" />
      </g>

      {/* ---- reach line: the hand almost touches the bloom (violet) ---- */}
      <path
        className="draw-path"
        pathLength={1}
        d="M470 226 C 466 216, 462 208, 456 202"
        stroke="var(--violet)"
        strokeWidth="3"
        strokeLinecap="round"
        strokeDasharray="2 7"
        opacity="0.8"
      />
      {/* drifting spark between hand and flower */}
      <path
        d="M480 200 C 484 194, 484 188, 480 184 C 486 187, 490 190, 494 190 C 490 193, 486 196, 484 200 C 484 196, 482 197, 480 200 Z"
        fill="var(--gold)"
      />
    </svg>
  );
}
