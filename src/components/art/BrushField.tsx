type BrushFieldProps = {
  className?: string;
};

/**
 * Brush field — irregular painted color blobs, three layered tones of
 * the one mural color (violet: the human layer). Sits behind content,
 * bleeding across a section edge. Never symmetric, never a wallpaper.
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
      {/* widest, faintest wash */}
      <path
        d="M-80 120 C 60 60, 260 90, 330 190 C 400 288, 340 380, 420 452 C 496 518, 470 620, 360 668 C 240 720, 60 700, -60 640 Z"
        fill="var(--violet)"
        opacity="0.10"
      />
      {/* mid tone, offset — the stroke of a wide brush */}
      <path
        d="M-90 210 C 20 150, 190 170, 250 260 C 306 344, 250 420, 316 488 C 372 546, 330 620, 230 646 C 120 674, -20 650, -90 600 Z"
        fill="var(--violet)"
        opacity="0.16"
      />
      {/* densest core, smallest, clearly off-center */}
      <path
        d="M-100 320 C -20 270, 110 284, 156 350 C 200 412, 160 470, 208 520 C 248 562, 210 610, 130 622 C 40 636, -60 610, -100 570 Z"
        fill="var(--violet)"
        opacity="0.22"
      />
      {/* a flick of the brush, escaping the field */}
      <path
        d="M300 156 C 356 130, 420 128, 470 148 C 430 152, 380 162, 338 182 C 322 174, 310 164, 300 156 Z"
        fill="var(--violet)"
        opacity="0.28"
      />
    </svg>
  );
}
