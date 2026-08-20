import React from "react";
import { Plan, ChecklistItem } from "../types/plan";
import { VertebraCard } from "./VertebraCard";
import { CheckCircle2, ChevronRight, Sparkles, BookOpen } from "lucide-react";

interface SpineBoardProps {
  plan: Plan;
  onToggleItem: (vertebraId: string, itemIndex: number, field: "built" | "tested") => void;
  onViewProof: (item: ChecklistItem, vertebraTitle: string) => void;
  onApproveSpine: () => void;
}

export const SpineBoard: React.FC<SpineBoardProps> = ({
  plan,
  onToggleItem,
  onViewProof,
  onApproveSpine
}) => {
  const isSpineApproved = plan.approved === "yes";

  return (
    <div className="flex-1 overflow-y-auto p-8 relative">
      {/* Spine Approval Banner if not approved */}
      {!isSpineApproved && (
        <div className="mb-8 p-4 rounded-2xl bg-gradient-to-r from-amber-950/70 via-slate-900 to-indigo-950/70 border border-amber-500/50 flex items-center justify-between shadow-xl">
          <div className="flex items-center gap-3">
            <div className="w-10 h-10 rounded-xl bg-amber-500/20 border border-amber-400/40 flex items-center justify-center text-amber-300">
              <BookOpen className="w-5 h-5" />
            </div>
            <div>
              <h3 className="text-sm font-bold text-amber-200">Spine Phase Review Required</h3>
              <p className="text-xs text-slate-300">Review the spinal chapters below before executing fine-detail vertebrae.</p>
            </div>
          </div>

          <button
            onClick={onApproveSpine}
            className="px-4 py-2 text-xs font-bold text-slate-900 bg-amber-400 hover:bg-amber-300 rounded-xl transition shadow-lg shadow-amber-500/20 flex items-center gap-2"
          >
            <CheckCircle2 className="w-4 h-4" />
            <span>APPROVE SPINE</span>
          </button>
        </div>
      )}

      {/* Spinal Phases & Vertebrae Branches */}
      <div className="space-y-12 max-w-6xl mx-auto relative">
        {/* Central Spine line running vertically */}
        <div className="absolute left-1/2 top-4 bottom-4 w-1 -translate-x-1/2 spine-line hidden lg:block rounded-full z-0 opacity-60"></div>

        {plan.spine.map((phase, phaseIdx) => {
          const leftVertebrae = plan.vertebrae.filter(v => v.spineId === phase.id && v.side === "L");
          const rightVertebrae = plan.vertebrae.filter(v => v.spineId === phase.id && v.side === "R");
          // If no side specified, distribute evenly
          const unassigned = plan.vertebrae.filter(v => v.spineId === phase.id && !v.side);
          unassigned.forEach((v, i) => {
            if (i % 2 === 0) leftVertebrae.push(v);
            else rightVertebrae.push(v);
          });

          return (
            <div key={phase.id} className="relative z-10 space-y-6">
              {/* Spine Node (The Chapter Center) */}
              <div className="flex justify-center">
                <div className="glass-card px-6 py-3 rounded-2xl border border-indigo-500/50 bg-[#161b22]/90 shadow-xl flex items-center gap-3 backdrop-blur-md">
                  <div className="w-7 h-7 rounded-lg bg-indigo-600/30 border border-indigo-400 flex items-center justify-center font-mono font-bold text-xs text-indigo-300">
                    {phase.id}
                  </div>
                  <div>
                    <h2 className="text-sm font-extrabold text-slate-100 tracking-tight flex items-center gap-2">
                      <span>Chapter {phaseIdx + 1}: {phase.title}</span>
                    </h2>
                    <p className="text-xs text-slate-400">{phase.summary}</p>
                  </div>
                </div>
              </div>

              {/* Vertebrae Left & Right Layout */}
              <div className="grid grid-cols-1 lg:grid-cols-2 gap-8 items-start">
                {/* Left Branch */}
                <div className="space-y-4">
                  {leftVertebrae.length > 0 ? (
                    leftVertebrae.map(vert => (
                      <VertebraCard
                        key={vert.id}
                        vertebra={vert}
                        onToggleItem={onToggleItem}
                        onViewProof={onViewProof}
                      />
                    ))
                  ) : (
                    <div className="hidden lg:block h-12"></div>
                  )}
                </div>

                {/* Right Branch */}
                <div className="space-y-4">
                  {rightVertebrae.length > 0 ? (
                    rightVertebrae.map(vert => (
                      <VertebraCard
                        key={vert.id}
                        vertebra={vert}
                        onToggleItem={onToggleItem}
                        onViewProof={onViewProof}
                      />
                    ))
                  ) : (
                    <div className="hidden lg:block h-12"></div>
                  )}
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
};
