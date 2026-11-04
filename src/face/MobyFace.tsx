import type { Expression } from "@mobyuz/moby-protocol";
import type { ReactNode } from "react";
import { EXPRESSION_META } from "./expressions";

const LEFT = 84;
const RIGHT = 172;
const EYE_Y = 56;

function RoundEyes({ width, height, dx = 0, dy = 0 }: { width: number; height: number; dx?: number; dy?: number }) {
  return (
    <g className="eyes">
      {[LEFT, RIGHT].map((cx) => (
        <rect
          key={cx}
          className="eye"
          x={cx + dx - width / 2}
          y={EYE_Y + dy - height / 2}
          width={width}
          height={height}
          rx={width / 2}
        />
      ))}
    </g>
  );
}

function ArcEyes({ lift = 0 }: { lift?: number }) {
  return (
    <g className="eyes">
      {[LEFT, RIGHT].map((cx) => (
        <path
          key={cx}
          className="stroke"
          d={`M ${cx - 20} ${EYE_Y + 10 - lift} Q ${cx} ${EYE_Y - 22 - lift} ${cx + 20} ${EYE_Y + 10 - lift}`}
        />
      ))}
    </g>
  );
}

function ClosedEyes() {
  return (
    <g className="eyes">
      {[LEFT, RIGHT].map((cx) => (
        <path key={cx} className="stroke" d={`M ${cx - 20} ${EYE_Y} Q ${cx} ${EYE_Y + 12} ${cx + 20} ${EYE_Y}`} />
      ))}
    </g>
  );
}

function CrossEyes() {
  return (
    <g className="eyes">
      {[LEFT, RIGHT].map((cx) => (
        <path
          key={cx}
          className="stroke"
          d={`M ${cx - 14} ${EYE_Y - 14} L ${cx + 14} ${EYE_Y + 14} M ${cx + 14} ${EYE_Y - 14} L ${cx - 14} ${EYE_Y + 14}`}
        />
      ))}
    </g>
  );
}

function Features({ expression }: { expression: Expression }): ReactNode {
  switch (expression) {
    case "idle":
      return <RoundEyes width={28} height={40} />;
    case "listening":
      return (
        <>
          <RoundEyes width={36} height={50} />
          <rect className="ring" x={6} y={6} width={244} height={116} rx={24} />
        </>
      );
    case "thinking":
      return (
        <>
          <RoundEyes width={24} height={30} dx={8} dy={-6} />
          <g className="dots">
            {[0, 1, 2].map((i) => (
              <circle key={i} className="dot" cx={112 + i * 16} cy={102} r={4} />
            ))}
          </g>
        </>
      );
    case "speaking":
      return (
        <>
          <RoundEyes width={28} height={34} dy={-8} />
          <g className="bars">
            {[0, 1, 2, 3, 4].map((i) => (
              <rect key={i} className="bar" x={96 + i * 14} y={92} width={8} height={24} rx={4} />
            ))}
          </g>
        </>
      );
    case "happy":
      return <ArcEyes />;
    case "confused":
      return (
        <>
          <RoundEyes width={36} height={48} dx={-6} />
          <rect className="eye tilted" x={RIGHT - 26} y={EYE_Y - 8} width={22} height={16} rx={8} />
          <text className="mark" x={214} y={36}>
            ?
          </text>
        </>
      );
    case "error":
      return (
        <>
          <CrossEyes />
          <path className="stroke thin" d="M 100 100 L 112 94 L 124 106 L 136 94 L 148 106 L 156 100" />
        </>
      );
    case "muted":
      return (
        <>
          <ClosedEyes />
          <g className="mic">
            <rect className="stroke thin" x={120} y={84} width={16} height={26} rx={8} />
            <path className="stroke thin" d="M 114 104 Q 128 122 142 104" />
            <path className="slash" d="M 110 118 L 146 80" />
          </g>
        </>
      );
    case "disconnected":
      return (
        <>
          <g className="eyes">
            {[LEFT, RIGHT].map((cx) => (
              <rect
                key={cx}
                className="eye outline"
                x={cx - 14}
                y={EYE_Y - 20}
                width={28}
                height={40}
                rx={14}
              />
            ))}
          </g>
          <g className="link">
            <path className="stroke thin" d="M 108 104 h 10 a 8 8 0 0 0 0 -16 h -4 M 148 88 h -10 a 8 8 0 0 0 0 16 h 4" />
            <path className="slash" d="M 122 112 L 134 80" />
          </g>
        </>
      );
  }
}

/** Moby's OLED screen, the same face the robot shows. */
export function MobyFace({ expression }: { expression: Expression }) {
  const meta = EXPRESSION_META[expression];
  return (
    <svg className="robot" data-expression={expression} data-animation={meta.animation} viewBox="0 0 256 128" role="img" aria-label={`Moby is ${meta.label.toLowerCase()}`}>
      <rect className="robot-screen" width={256} height={128} rx={36} />
      <g className="face">
        <Features expression={expression} />
      </g>
    </svg>
  );
}
