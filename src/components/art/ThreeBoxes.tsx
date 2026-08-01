/**
 * "Two binaries plus Git" — the whole system, drawn as three
 * hand-sketched boxes (brief §8.2). The sketch annotates the principle
 * beside it; the accessible description is the adjacent panel text.
 */
export function ThreeBoxes({ className }: { className?: string }) {
  return (
    <svg aria-hidden viewBox="0 0 560 260" fill="none" className={className}>
      {/* data plane box — wobbly rect */}
      <path
        className="draw-path"
        pathLength={1}
        d="M22 78 C 74 74, 128 75, 172 79 C 175 106, 174 134, 171 158 C 122 162, 70 161, 24 157 C 20 132, 20 104, 22 78 Z"
        stroke="var(--violet)"
        strokeWidth="2.5"
        strokeLinecap="round"
      />
      {/* control plane box */}
      <path
        className="draw-path"
        pathLength={1}
        d="M216 76 C 268 73, 322 74, 366 80 C 370 106, 369 134, 365 160 C 316 163, 264 162, 218 157 C 214 131, 213 103, 216 76 Z"
        stroke="var(--violet)"
        strokeWidth="2.5"
        strokeLinecap="round"
      />
      {/* git box — drawn slightly apart, the source of truth */
      }
      <path
        className="draw-path"
        pathLength={1}
        d="M424 76 C 466 72, 506 74, 540 79 C 543 105, 542 133, 539 158 C 502 162, 462 161, 426 157 C 422 131, 421 103, 424 76 Z"
        stroke="var(--violet)"
        strokeWidth="2.5"
        strokeLinecap="round"
      />
      {/* snapshot stream: control plane -> data plane */}
      <path
        className="draw-path"
        pathLength={1}
        d="M212 108 C 200 106, 190 106, 178 108 M 188 102 L 176 108 L 188 115"
        stroke="var(--violet)"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      {/* truth: git -> control plane */}
      <path
        className="draw-path"
        pathLength={1}
        d="M418 108 C 404 106, 392 106, 372 108 M 384 102 L 370 108 L 384 115"
        stroke="var(--violet)"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      {/* labels — the instrument voice */}
      <text
        x="97"
        y="123"
        textAnchor="middle"
        className="voice-mono"
        fontSize="15"
        fill="var(--ink)"
      >
        data plane
      </text>
      <text
        x="291"
        y="123"
        textAnchor="middle"
        className="voice-mono"
        fontSize="15"
        fill="var(--ink)"
      >
        control plane
      </text>
      <text
        x="482"
        y="123"
        textAnchor="middle"
        className="voice-mono"
        fontSize="15"
        fill="var(--ink)"
      >
        git
      </text>
      {/* sub-captions */}
      <text
        x="97"
        y="185"
        textAnchor="middle"
        className="voice-mono"
        fontSize="11"
        fill="var(--steel-dark)"
      >
        one binary
      </text>
      <text
        x="291"
        y="185"
        textAnchor="middle"
        className="voice-mono"
        fontSize="11"
        fill="var(--steel-dark)"
      >
        one binary + postgres
      </text>
      <text
        x="482"
        y="185"
        textAnchor="middle"
        className="voice-mono"
        fontSize="11"
        fill="var(--steel-dark)"
      >
        truth
      </text>
      {/* the hand's note */}
      <text
        x="291"
        y="34"
        textAnchor="middle"
        className="voice-hand"
        fontSize="19"
        fill="var(--violet)"
        transform="rotate(-2 291 34)"
      >
        the whole system
      </text>
    </svg>
  );
}
