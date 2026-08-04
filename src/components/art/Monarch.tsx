type MonarchProps = {
  className?: string;
};

/**
 * The monarch — one butterfly, used exactly once per site (brief §4),
 * at the moment of transformation: here, the ownership contract.
 * Hand-drawn path quality; monarch orange + floor-dark.
 */
export function Monarch({ className }: MonarchProps) {
  return (
    <svg aria-hidden viewBox="0 0 220 200" fill="none" className={className}>
      {/* left forewing */}
      <path
        d="M104 96 C 84 62, 52 30, 26 26 C 6 23, 2 42, 10 62 C 20 87, 52 106, 92 108 C 98 105, 102 101, 104 96 Z"
        fill="var(--monarch)"
        stroke="var(--floor)"
        strokeWidth="3"
        strokeLinejoin="round"
      />
      {/* left hindwing */}
      <path
        d="M96 112 C 66 112, 38 124, 30 146 C 24 165, 40 178, 62 172 C 84 166, 100 144, 104 120 C 101 117, 98 114, 96 112 Z"
        fill="var(--monarch)"
        stroke="var(--floor)"
        strokeWidth="3"
        strokeLinejoin="round"
      />
      {/* right forewing */}
      <path
        d="M116 96 C 136 60, 170 28, 196 26 C 216 25, 218 45, 208 64 C 196 88, 164 106, 128 108 C 122 105, 118 101, 116 96 Z"
        fill="var(--monarch)"
        stroke="var(--floor)"
        strokeWidth="3"
        strokeLinejoin="round"
      />
      {/* right hindwing */}
      <path
        d="M124 112 C 154 114, 180 126, 188 148 C 194 166, 178 178, 156 172 C 136 166, 120 144, 116 120 C 119 117, 122 114, 124 112 Z"
        fill="var(--monarch)"
        stroke="var(--floor)"
        strokeWidth="3"
        strokeLinejoin="round"
      />
      {/* wing veins — the hand's linework */}
      <path
        d="M98 94 C 78 76, 56 56, 34 40 M 94 102 C 72 96, 50 92, 30 92 M 96 118 C 76 130, 60 144, 48 160 M 122 94 C 142 74, 164 54, 188 40 M 126 102 C 148 96, 170 92, 192 94 M 124 118 C 144 130, 160 144, 172 160"
        stroke="var(--floor)"
        strokeWidth="2"
        strokeLinecap="round"
        opacity="0.75"
      />
      {/* body */}
      <path
        d="M110 84 C 106 96, 105 122, 108 140 C 109 147, 113 147, 114 140 C 116 122, 115 96, 112 84 C 111 82, 111 82, 110 84 Z"
        fill="var(--floor)"
      />
      {/* antennae */}
      <path
        d="M108 84 C 102 72, 94 62, 84 56 M 113 84 C 118 72, 126 62, 136 56"
        stroke="var(--floor)"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}
