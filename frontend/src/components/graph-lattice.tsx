/**
 * GraphLattice — decorative SVG node/edge lattice used as the background of
 * the auth screens. Evokes the product's core metaphor (the knowledge graph
 * without baking in any one schema). Coordinates are fixed (deterministic) so
 * the server-rendered and client-rendered markup match — no hydration drift.
 *
 * Purely presentational, hidden from assistive tech via `aria-hidden`.
 */
const NODES: Array<{ x: number; y: number; r: number; strong?: boolean; delay: number }> = [
  { x: 120, y: 90, r: 3.5, strong: true, delay: 0.05 },
  { x: 270, y: 60, r: 2.5, delay: 0.12 },
  { x: 410, y: 130, r: 3, delay: 0.18 },
  { x: 560, y: 70, r: 2.5, delay: 0.22 },
  { x: 720, y: 110, r: 3.5, strong: true, delay: 0.28 },
  { x: 180, y: 220, r: 2.5, delay: 0.32 },
  { x: 340, y: 250, r: 3, delay: 0.38 },
  { x: 480, y: 210, r: 2.5, delay: 0.42 },
  { x: 640, y: 260, r: 3, delay: 0.48 },
  { x: 780, y: 220, r: 2.5, delay: 0.54 },
  { x: 100, y: 380, r: 3, delay: 0.6 },
  { x: 280, y: 420, r: 2.5, delay: 0.64 },
  { x: 460, y: 390, r: 3.5, strong: true, delay: 0.68 },
  { x: 620, y: 430, r: 2.5, delay: 0.72 },
  { x: 760, y: 380, r: 3, delay: 0.78 },
];

const EDGES: Array<[number, number]> = [
  [0, 1],
  [1, 2],
  [2, 3],
  [3, 4],
  [0, 5],
  [5, 6],
  [6, 2],
  [6, 7],
  [7, 3],
  [7, 8],
  [8, 4],
  [8, 9],
  [5, 10],
  [10, 11],
  [11, 6],
  [6, 12],
  [12, 7],
  [12, 13],
  [13, 8],
  [13, 14],
  [14, 9],
  [10, 14],
];

export interface GraphLatticeProps {
  className?: string;
}

export function GraphLattice({ className }: GraphLatticeProps) {
  return (
    <svg
      aria-hidden="true"
      className={`auth-lattice ${className ?? ""}`}
      viewBox="0 0 860 480"
      preserveAspectRatio="xMidYMid slice"
    >
      <g>
        {EDGES.map(([a, b], i) => {
          const na = NODES[a];
          const nb = NODES[b];
          return (
            <line
              key={`edge-${i}`}
              className="lattice-edge"
              x1={na.x}
              y1={na.y}
              x2={nb.x}
              y2={nb.y}
              style={{ animationDelay: `${0.05 + i * 0.04}s` }}
            />
          );
        })}
      </g>
      <g>
        {NODES.map((n, i) => (
          <circle
            key={`node-${i}`}
            className={`lattice-node ${n.strong ? "lattice-node-strong" : ""}`}
            cx={n.x}
            cy={n.y}
            r={n.r}
            style={{ animationDelay: `${n.delay}s` }}
          />
        ))}
      </g>
    </svg>
  );
}