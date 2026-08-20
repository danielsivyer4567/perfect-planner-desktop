import React from "react";
import { Vertebra, ChecklistItem } from "../types/plan";
import { CheckSquare, Square, Shield, FileCode, Lock, Zap, Eye, AlertOctagon } from "lucide-react";

interface VertebraCardProps {
  vertebra: Vertebra;
  onToggleItem: (vertebraId: string, itemIndex: number, field: "built" | "tested") => void;
  onViewProof: (item: ChecklistItem, vertebraTitle: string) => void;
}

export const VertebraCard: React.FC<VertebraCardProps> = ({
  vertebra,
  onToggleItem,
  onViewProof
}) => {
  const isComplete = vertebra.status === "complete";
  const isInProgress = vertebra.status === "in-progress";

  const total = vertebra.checklist.length;
  const builtCount = vertebra.checklist.filter(c => c.built).length;
  const testedCount = vertebra.checklist.filter(c => c.tested).length;

  return (
    <div className={`glass-card rounded-2xl p-4 transition-all duration-200 border relative ${
      isComplete
        ? "border-emerald-800/60 bg-emerald-950/10"
        : isInProgress
        ? "border-indigo-700/60 bg-indigo-950/10"
        : "border-slate-800 bg-[#161b22]/70"
    }`}>
      {/* Header */}
      <div className="flex items-start justify-between gap-3 pb-3 border-b border-slate-800/80">
        <div className="flex items-center gap-2">
          <span className="font-mono text-xs font-bold px-2 py-0.5 rounded bg-slate-800 text-indigo-400 border border-slate-700">
            {vertebra.id}
          </span>
          <h4 className="text-xs font-bold text-slate-100">{vertebra.title}</h4>
        </div>

        <div className="flex items-center gap-2">
          {vertebra.est && (
            <span className="text-[10px] font-mono text-slate-400 bg-slate-800/80 px-2 py-0.5 rounded border border-slate-700/60">
              ~{(vertebra.est.tokens || 0) / 1000}k tok • {vertebra.est.minutes}m
            </span>
          )}
          <span className={`text-[10px] uppercase font-bold px-2 py-0.5 rounded border ${
            isComplete
              ? "bg-emerald-950 text-emerald-300 border-emerald-700"
              : isInProgress
              ? "bg-indigo-950 text-indigo-300 border-indigo-700"
              : "bg-slate-800 text-slate-400 border-slate-700"
          }`}>
            {vertebra.status}
          </span>
        </div>
      </div>

      {/* Collision Gates Warning if any */}
      {vertebra.gates && vertebra.gates.length > 0 && (
        <div className="mt-2.5 p-2 rounded-lg bg-amber-950/40 border border-amber-800/50 text-[11px] text-amber-200 space-y-1">
          {vertebra.gates.map((g, idx) => (
            <div key={idx} className="flex items-center gap-1.5">
              <AlertOctagon className="w-3.5 h-3.5 text-amber-400 shrink-0" />
              <span>Gate ({g.status}): {g.clearNote || `Collision with ${g.withPlan} ${g.withVertebra}`}</span>
            </div>
          ))}
        </div>
      )}

      {/* Files write set */}
      {vertebra.files && vertebra.files.length > 0 && (
        <div className="mt-2.5 flex items-center gap-1.5 flex-wrap">
          <FileCode className="w-3 h-3 text-slate-500 shrink-0" />
          {vertebra.files.map((f, idx) => (
            <span key={idx} className="text-[10px] font-mono text-slate-400 bg-slate-800/60 px-1.5 py-0.5 rounded border border-slate-700/50 truncate max-w-[200px]">
              {f}
            </span>
          ))}
        </div>
      )}

      {/* Dot Points / Checklist Items */}
      <div className="mt-3.5 space-y-2.5">
        {vertebra.checklist.map((item, idx) => (
          <div
            key={idx}
            className="p-2.5 rounded-xl bg-slate-900/60 hover:bg-slate-900/90 border border-slate-800/70 transition flex flex-col gap-2"
          >
            <div className="flex items-start justify-between gap-3">
              <div className="text-xs text-slate-200 leading-snug flex-1">
                {item.text}
                {item.ui && (
                  <span className="ml-1.5 inline-flex items-center gap-0.5 text-[9px] font-mono text-cyan-300 bg-cyan-950 px-1.5 py-0.2 rounded border border-cyan-800">
                    <Eye className="w-2.5 h-2.5" /> UI
                  </span>
                )}
              </div>

              {/* Dual Checkbox Controls */}
              <div className="flex items-center gap-3 shrink-0">
                {/* Built checkbox */}
                <button
                  onClick={() => onToggleItem(vertebra.id, idx, "built")}
                  className={`flex items-center gap-1 text-[11px] font-mono px-2 py-1 rounded transition ${
                    item.built
                      ? "bg-cyan-950/80 text-cyan-300 border border-cyan-700/80"
                      : "bg-slate-800/60 text-slate-400 hover:text-slate-200 border border-slate-700/50"
                  }`}
                  title="Toggle Built"
                >
                  {item.built ? <CheckSquare className="w-3.5 h-3.5 text-cyan-400" /> : <Square className="w-3.5 h-3.5" />}
                  <span>Built</span>
                </button>

                {/* Tested checkbox */}
                <button
                  onClick={() => onToggleItem(vertebra.id, idx, "tested")}
                  className={`flex items-center gap-1 text-[11px] font-mono px-2 py-1 rounded transition ${
                    item.tested
                      ? "bg-emerald-950/80 text-emerald-300 border border-emerald-700/80"
                      : "bg-slate-800/60 text-slate-400 hover:text-slate-200 border border-slate-700/50"
                  }`}
                  title="Toggle Tested (with captured proof)"
                >
                  {item.tested ? <CheckSquare className="w-3.5 h-3.5 text-emerald-400" /> : <Square className="w-3.5 h-3.5" />}
                  <span>Tested</span>
                </button>
              </div>
            </div>

            {/* Proof pill or verify command */}
            <div className="flex items-center justify-between text-[10px] font-mono text-slate-400 pt-1 border-t border-slate-800/50">
              <span className="truncate max-w-[220px] text-slate-400">
                {item.verify ? `cmd: ${item.verify}` : item.ref ? `ref: ${item.ref}` : "Manual check"}
              </span>

              {item.proof ? (
                <button
                  onClick={() => onViewProof(item, vertebra.title)}
                  className="flex items-center gap-1 text-emerald-400 bg-emerald-950/60 hover:bg-emerald-900/60 border border-emerald-700/60 px-2 py-0.5 rounded transition"
                >
                  <Shield className="w-3 h-3 text-emerald-400" />
                  <span>Proof: {item.proof.by}</span>
                </button>
              ) : (
                <span className="text-slate-500 italic">No proof captured</span>
              )}
            </div>
          </div>
        ))}
      </div>

      {/* Footer statistics */}
      <div className="mt-3 pt-2.5 border-t border-slate-800/80 flex items-center justify-between text-[11px] font-mono text-slate-400">
        <span>Items: {total}</span>
        <div className="flex gap-3">
          <span className="text-cyan-400">Built: {builtCount}/{total}</span>
          <span className="text-emerald-400">Tested: {testedCount}/{total}</span>
        </div>
      </div>
    </div>
  );
};
