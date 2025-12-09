import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import type { TxIn, TxOut } from "@subcoin/shared";
import { detectScriptType } from "@subcoin/shared";

interface TransactionSankeyProps {
  inputs: TxIn[];
  outputs: TxOut[];
  inputValues?: number[]; // Values for inputs (if available from previous outputs)
  isCoinbase?: boolean;
  /** Callback to resolve input addresses */
  resolveInputAddress?: (txid: string, vout: number) => Promise<string | null>;
}

interface InputNode {
  index: number;
  value: number;
  label: string;
  txid?: string;
  vout?: number;
  isCoinbase: boolean;
}

interface OutputNode {
  index: number;
  value: number;
  label: string;
  scriptType: string;
  colorClass: string;
}

function formatBtc(satoshis: number): string {
  return (satoshis / 100_000_000).toFixed(8);
}

function shortenTxid(txid: string): string {
  return `${txid.slice(0, 12)}...${txid.slice(-6)}`;
}

/**
 * Pure SVG Sankey diagram for Bitcoin transaction visualization
 */
export function TransactionSankey({
  inputs,
  outputs,
  inputValues,
  isCoinbase = false,
}: TransactionSankeyProps) {
  const [hoveredFlow, setHoveredFlow] = useState<number | null>(null);

  // Calculate layout
  const layout = useMemo(() => {
    const totalOutput = outputs.reduce((sum, out) => sum + out.value, 0);

    // Build input nodes
    const inputNodes: InputNode[] = inputs.map((inp, i) => ({
      index: i,
      value: inputValues?.[i] ?? 0,
      label:
        isCoinbase && i === 0
          ? "Coinbase"
          : `${shortenTxid(inp.previous_output.txid)}:${inp.previous_output.vout}`,
      txid: inp.previous_output.txid,
      vout: inp.previous_output.vout,
      isCoinbase: isCoinbase && i === 0,
    }));

    // Build output nodes
    const outputNodes: OutputNode[] = outputs.map((out, i) => {
      const scriptInfo = detectScriptType(out.script_pubkey);
      return {
        index: i,
        value: out.value,
        label: `Output #${i}`,
        scriptType: scriptInfo.label,
        colorClass: scriptInfo.colorClass,
      };
    });

    // If we don't have input values, distribute evenly or use output total
    const totalInputValue = inputNodes.reduce((sum, n) => sum + n.value, 0);
    if (totalInputValue === 0 && inputNodes.length > 0) {
      // Distribute output total across inputs for visualization
      const perInput = totalOutput / inputNodes.length;
      inputNodes.forEach((n) => (n.value = perInput));
    }

    return { inputNodes, outputNodes, totalOutput };
  }, [inputs, outputs, inputValues, isCoinbase]);

  const { inputNodes, outputNodes, totalOutput } = layout;

  // SVG dimensions - adaptive layout
  const width = 600;
  const targetHeight = 180; // Target total height for the diagram
  const nodeGap = 8;
  const minNodeHeight = 36;
  const maxNodeHeight = 60;
  const nodeWidth = 150;
  const leftX = 10;
  const rightX = width - nodeWidth - 10;

  // Calculate node heights to fill available space, respecting min/max
  const calculateNodeHeight = (nodeCount: number) => {
    if (nodeCount === 0) return minNodeHeight;
    const availableForNodes = targetHeight - 30 - (nodeCount - 1) * nodeGap;
    const idealHeight = availableForNodes / nodeCount;
    return Math.min(maxNodeHeight, Math.max(minNodeHeight, idealHeight));
  };

  const inputNodeHeight = calculateNodeHeight(inputNodes.length);
  const outputNodeHeight = calculateNodeHeight(outputNodes.length);

  // Calculate actual heights needed for each side
  const inputsTotalHeight = inputNodes.length > 0
    ? inputNodes.length * inputNodeHeight + (inputNodes.length - 1) * nodeGap
    : 0;
  const outputsTotalHeight = outputNodes.length > 0
    ? outputNodes.length * outputNodeHeight + (outputNodes.length - 1) * nodeGap
    : 0;
  const height = Math.max(inputsTotalHeight, outputsTotalHeight) + 40;

  // Heights for each node
  const inputHeights = inputNodes.map(() => inputNodeHeight);
  const outputHeights = outputNodes.map(() => outputNodeHeight);

  // Calculate Y positions - center each side independently, with top padding for labels
  const topPadding = 28; // Space for labels
  const calculateYPositions = (heights: number[]) => {
    const totalHeight = heights.reduce((sum, h) => sum + h + nodeGap, 0) - (heights.length > 0 ? nodeGap : 0);
    // Center in the area below the labels
    const availableHeight = height - topPadding;
    let startY = topPadding + (availableHeight - totalHeight) / 2;
    const positions: number[] = [];
    heights.forEach((h) => {
      positions.push(startY);
      startY += h + nodeGap;
    });
    return positions;
  };

  const inputYs = calculateYPositions(inputHeights);
  const outputYs = calculateYPositions(outputHeights);

  // Generate flow paths (curved bezier)
  const generateFlowPath = (
    inputIdx: number,
    outputIdx: number,
    flowHeight: number
  ) => {
    const x1 = leftX + nodeWidth;
    const y1 = inputYs[inputIdx] + inputHeights[inputIdx] / 2;
    const x2 = rightX;
    const y2 = outputYs[outputIdx] + outputHeights[outputIdx] / 2;

    const controlX1 = x1 + (x2 - x1) * 0.4;
    const controlX2 = x1 + (x2 - x1) * 0.6;

    // Create a thick path using bezier curve
    const halfHeight = flowHeight / 2;
    return `
      M ${x1} ${y1 - halfHeight}
      C ${controlX1} ${y1 - halfHeight}, ${controlX2} ${y2 - halfHeight}, ${x2} ${y2 - halfHeight}
      L ${x2} ${y2 + halfHeight}
      C ${controlX2} ${y2 + halfHeight}, ${controlX1} ${y1 + halfHeight}, ${x1} ${y1 + halfHeight}
      Z
    `;
  };

  // Simple flow distribution: each input connects to all outputs proportionally
  const flows = useMemo(() => {
    const result: Array<{
      inputIdx: number;
      outputIdx: number;
      value: number;
      path: string;
    }> = [];

    if (inputNodes.length === 0 || outputNodes.length === 0) return result;

    // For simplicity: distribute each input across all outputs based on output proportion
    inputNodes.forEach((input, iIdx) => {
      outputNodes.forEach((output, oIdx) => {
        const flowValue =
          totalOutput > 0
            ? (input.value * output.value) / totalOutput
            : input.value / outputNodes.length;

        const flowHeight = Math.max(
          4,
          totalOutput > 0 ? (flowValue / totalOutput) * height * 0.4 : 8
        );

        result.push({
          inputIdx: iIdx,
          outputIdx: oIdx,
          value: flowValue,
          path: generateFlowPath(iIdx, oIdx, flowHeight),
        });
      });
    });

    return result;
  }, [inputNodes, outputNodes, totalOutput, inputYs, outputYs, inputHeights, outputHeights]);

  // Color palette for flows
  const flowColors = [
    "rgba(251, 146, 60, 0.4)", // orange
    "rgba(59, 130, 246, 0.4)", // blue
    "rgba(34, 197, 94, 0.4)", // green
    "rgba(168, 85, 247, 0.4)", // purple
    "rgba(236, 72, 153, 0.4)", // pink
    "rgba(20, 184, 166, 0.4)", // teal
  ];

  const getFlowColor = (inputIdx: number, hovered: boolean) => {
    const baseColor = flowColors[inputIdx % flowColors.length];
    if (hovered) {
      return baseColor.replace("0.4)", "0.7)");
    }
    return baseColor;
  };

  return (
    <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-3">
      <h3 className="text-sm font-medium text-gray-100 mb-2">
        Transaction Flow
      </h3>

      <svg
        width="100%"
        viewBox={`0 0 ${width} ${height}`}
        className="overflow-visible max-w-lg"
      >
        {/* Flow paths */}
        {flows.map((flow, idx) => (
          <path
            key={idx}
            d={flow.path}
            fill={getFlowColor(
              flow.inputIdx,
              hoveredFlow === flow.inputIdx
            )}
            stroke={
              hoveredFlow === flow.inputIdx
                ? "rgba(251, 146, 60, 0.8)"
                : "rgba(255,255,255,0.1)"
            }
            strokeWidth={hoveredFlow === flow.inputIdx ? 1 : 0.5}
            className="transition-all duration-200"
            onMouseEnter={() => setHoveredFlow(flow.inputIdx)}
            onMouseLeave={() => setHoveredFlow(null)}
          />
        ))}

        {/* Input nodes */}
        {inputNodes.map((node, idx) => (
          <g key={`input-${idx}`}>
            {/* Node rectangle */}
            <rect
              x={leftX}
              y={inputYs[idx]}
              width={nodeWidth}
              height={inputHeights[idx]}
              rx={4}
              fill={node.isCoinbase ? "rgba(251, 146, 60, 0.3)" : "rgba(59, 130, 246, 0.3)"}
              stroke={node.isCoinbase ? "rgb(251, 146, 60)" : "rgb(59, 130, 246)"}
              strokeWidth={1}
              className="cursor-pointer hover:fill-opacity-50 transition-all"
              onMouseEnter={() => setHoveredFlow(idx)}
              onMouseLeave={() => setHoveredFlow(null)}
            />

            {/* Node label */}
            <foreignObject
              x={leftX + 4}
              y={inputYs[idx] + 4}
              width={nodeWidth - 8}
              height={inputHeights[idx] - 8}
            >
              <div className="h-full flex flex-col justify-center text-xs">
                {node.isCoinbase ? (
                  <span className="text-bitcoin-orange font-medium truncate">
                    Coinbase
                  </span>
                ) : (
                  <Link
                    to={`/tx/${node.txid}`}
                    className="text-blue-400 hover:text-blue-300 font-mono truncate"
                  >
                    {shortenTxid(node.txid || "")}
                  </Link>
                )}
                {node.value > 0 && (
                  <span className="text-gray-400 font-mono text-[10px]">
                    {formatBtc(node.value)} BTC
                  </span>
                )}
              </div>
            </foreignObject>
          </g>
        ))}

        {/* Output nodes */}
        {outputNodes.map((node, idx) => {
          const scriptInfo = detectScriptType(outputs[idx].script_pubkey);
          const isOpReturn = scriptInfo.type === "OP_RETURN";

          return (
            <g key={`output-${idx}`}>
              {/* Node rectangle */}
              <rect
                x={rightX}
                y={outputYs[idx]}
                width={nodeWidth}
                height={outputHeights[idx]}
                rx={4}
                fill={isOpReturn ? "rgba(107, 114, 128, 0.3)" : "rgba(34, 197, 94, 0.3)"}
                stroke={isOpReturn ? "rgb(107, 114, 128)" : "rgb(34, 197, 94)"}
                strokeWidth={1}
                className="transition-all"
              />

              {/* Node label */}
              <foreignObject
                x={rightX + 4}
                y={outputYs[idx] + 4}
                width={nodeWidth - 8}
                height={outputHeights[idx] - 8}
              >
                <div className="h-full flex flex-col justify-center text-xs">
                  <div className="flex items-center gap-1">
                    <span className="text-gray-300">#{idx}</span>
                    <span
                      className={`px-1 py-0.5 rounded text-[10px] ${scriptInfo.colorClass}`}
                    >
                      {scriptInfo.label}
                    </span>
                  </div>
                  <span
                    className={`font-mono text-[10px] ${
                      isOpReturn ? "text-gray-500" : "text-green-400"
                    }`}
                  >
                    {formatBtc(node.value)} BTC
                  </span>
                </div>
              </foreignObject>
            </g>
          );
        })}

        {/* Labels */}
        <text x={leftX} y={16} className="fill-gray-500 text-xs">
          Inputs ({inputs.length})
        </text>
        <text x={rightX} y={16} className="fill-gray-500 text-xs">
          Outputs ({outputs.length})
        </text>
      </svg>

      {/* Legend - compact */}
      <div className="mt-2 flex flex-wrap gap-3 text-[10px] text-gray-500">
        <div className="flex items-center gap-1">
          <div className="w-2 h-2 rounded bg-blue-500/30 border border-blue-500" />
          <span>Input</span>
        </div>
        <div className="flex items-center gap-1">
          <div className="w-2 h-2 rounded bg-green-500/30 border border-green-500" />
          <span>Output</span>
        </div>
        <div className="flex items-center gap-1">
          <div className="w-2 h-2 rounded bg-bitcoin-orange/30 border border-bitcoin-orange" />
          <span>Coinbase</span>
        </div>
        <div className="flex items-center gap-1">
          <div className="w-2 h-2 rounded bg-gray-500/30 border border-gray-500" />
          <span>OP_RETURN (data)</span>
        </div>
      </div>
    </div>
  );
}
