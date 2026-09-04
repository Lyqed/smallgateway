/**
 * The canonical event stream — the architecture centerpiece (brief §8.3):
 * six events as stations on one hand-drawn line crossing a machined
 * section. The SVG is decorative (aria-hidden); the ordered list below
 * it is the real content and is what screen readers get — it doubles as
 * the machined legend on desktop and the whole visual on small screens.
 */

const EVENTS = [
  {
    name: "MessageStart",
    note: "the stream opens",
  },
  {
    name: "ContentDelta",
    note: "text, token by token",
  },
  {
    name: "ToolCallDelta",
    note: "streamed tool calls, same contract as text",
  },
  {
    name: "UsageDelta",
    note: "the incremental tally mid-stream enforcement meters against",
  },
  {
    name: "MessageEnd",
    note: "always terminal; the usage frame precedes it",
  },
  {
    name: "Error",
    note: "an operator-defined terminal event, even mid-stream",
  },
] as const;

function Station({
  x,
  y,
  label,
  labelAbove,
}: {
  x: number;
  y: number;
  label: string;
  labelAbove: boolean;
}) {
  const labelY = labelAbove ? y - 34 : y + 48;
  const tickY1 = labelAbove ? y - 14 : y + 14;
  const tickY2 = labelAbove ? y - 26 : y + 26;
  return (
    <g>
      <line
        x1={x}
        y1={tickY1}
        x2={x}
        y2={tickY2}
        stroke="var(--steel)"
        strokeWidth="1.5"
      />
      <circle
        cx={x}
        cy={y}
        r="8"
        fill="var(--surface-panel)"
        stroke="var(--ink)"
        strokeWidth="2.5"
      />
      <text
        x={x}
        y={labelY}
        textAnchor="middle"
        className="voice-mono"
        fontSize="16"
        fill="var(--ink)"
      >
        {label}
      </text>
    </g>
  );
}

export function EventStream() {
  return (
    <figure className="m-0">
      {/* The drawn line — desktop only; the list below carries mobile. */}
      <svg
        aria-hidden
        viewBox="0 0 1160 250"
        fill="none"
        className="hidden w-full md:block"
      >
        {/* the hand-drawn line */}
        <path
          className="draw-path"
          pathLength={1}
          d="M28 134 C 110 126, 190 138, 280 132 C 370 126, 420 137, 500 133 C 580 129, 640 138, 712 134 C 790 130, 880 134, 962 132"
          stroke="var(--violet)"
          strokeWidth="2.5"
          strokeLinecap="round"
        />
        {/* the error branch, falling away from the line */}
        <path
          className="draw-path"
          pathLength={1}
          d="M804 133 C 878 148, 970 172, 1048 196"
          stroke="var(--blossom)"
          strokeWidth="2"
          strokeLinecap="round"
          strokeDasharray="7 6"
          opacity="0.85"
        />
        <Station x={70} y={132} label="MessageStart" labelAbove />
        <Station x={286} y={132} label="ContentDelta" labelAbove={false} />
        <Station x={502} y={132} label="ToolCallDelta" labelAbove />
        <Station x={716} y={132} label="UsageDelta" labelAbove={false} />
        {/* MessageEnd — terminal, filled */}
        <g>
          <line
            x1={962}
            y1={118}
            x2={962}
            y2={106}
            stroke="var(--steel)"
            strokeWidth="1.5"
          />
          <circle cx={962} cy={132} r="8" fill="var(--ink)" />
          <text
            x={962}
            y={98}
            textAnchor="middle"
            className="voice-mono"
            fontSize="16"
            fill="var(--ink)"
          >
            MessageEnd
          </text>
        </g>
        {/* Error — the other terminal, open blossom circle */}
        <g>
          <circle
            cx={1060}
            cy={199}
            r="8"
            fill="var(--surface-atrium)"
            stroke="var(--blossom)"
            strokeWidth="2.5"
          />
          <text
            x={1060}
            y={232}
            textAnchor="middle"
            className="voice-mono"
            fontSize="16"
            fill="var(--blossom)"
          >
            Error
          </text>
        </g>
        {/* the hand's note */}
        <text
          x="560"
          y="42"
          textAnchor="middle"
          className="voice-hand"
          fontSize="21"
          fill="var(--violet)"
          transform="rotate(-2 560 42)"
        >
          one contract, not three
        </text>
        <path
          className="draw-path"
          pathLength={1}
          d="M598 54 C 606 74, 608 94, 602 116 M 596 104 L 602 119 L 610 106"
          stroke="var(--violet)"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>

      {/* The readable stream — legend on desktop, the visual on mobile. */}
      <figcaption className="mt-6 md:mt-2">
        <ol
          aria-label="The six canonical events"
          className="grid gap-x-8 gap-y-4 border-t border-steel pt-6 sm:grid-cols-2 lg:grid-cols-3"
        >
          {EVENTS.map((event, i) => (
            <li key={event.name} className="flex gap-3">
              <span
                aria-hidden
                className="voice-mono pt-0.5 text-xs text-steel-dark"
              >
                {String(i + 1).padStart(2, "0")}
              </span>
              <span>
                <span
                  className={`voice-mono block text-sm font-medium ${
                    event.name === "Error" ? "text-blossom" : "text-ink"
                  }`}
                >
                  {event.name}
                </span>
                <span className="block text-sm text-steel-dark">
                  {event.note}
                </span>
              </span>
            </li>
          ))}
        </ol>
      </figcaption>
    </figure>
  );
}
