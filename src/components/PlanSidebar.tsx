import React from "react";
import { Plan } from "../types/plan";
import { PlanService } from "../services/planService";
import { Layers, Folder, Shield, CheckCircle, Clock, AlertTriangle } from "lucide-react";

interface PlanSidebarProps {
  plans: Plan[];
  activePlanId: string;
  onSelectPlan: (planId: string) => void;
  watchPath: string;
}

export const PlanSidebar: React.FC<PlanSidebarProps> = ({
  plans,
  activePlanId,
  onSelectPlan,
  watchPath
}) => {
  return (
    <aside className="w-80 h-full border-r border-[#30363d] bg-[#0d1117] flex flex-col justify-between shrink-0 select-none">
      {/* Top Branding */}
      <div className="p-4 border-b border-[#30363d]">
        <div className="flex items-center gap-2.5">
          <div className="w-8 h-8 rounded-lg bg-gradient-to-tr from-indigo-600 to-cyan-400 flex items-center justify-center shadow-md shadow-indigo-500/20">
            <Layers className="w-4 h-4 text-white" />
          </div>
          <div>
            <h1 className="text-sm font-bold text-slate-100 tracking-tight flex items-center gap-1.5">
              <span>Perfect Planner</span>
              <span className="text-[10px] bg-indigo-950 text-indigo-300 px-1.5 py-0.2 rounded border border-indigo-700">DESKTOP</span>
            </h1>
            <p className="text-[11px] text-slate-400 font-mono">By looplet • Live Spine Container</p>
          </div>
        </div>
      </div>

      {/* Plan Tabs List */}
      <div className="flex-1 overflow-y-auto p-3 space-y-2">
        <div className="px-2 py-1 text-[11px] font-semibold uppercase tracking-wider text-slate-500 flex items-center justify-between">
          <span>Active Plans ({plans.length})</span>
          <span className="text-[10px] text-emerald-400 flex items-center gap-1">
            <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse"></span>
            Auto-Sync
          </span>
        </div>

        {plans.map(plan => {
          const stats = PlanService.calculateStats(plan);
          const isActive = plan.id === activePlanId;
          const isComplete = stats.testedPercent === 100 && stats.builtPercent === 100;

          return (
            <div
              key={plan.id}
              onClick={() => onSelectPlan(plan.id)}
              className={`p-3 rounded-xl border transition-all cursor-pointer group relative ${
                isActive
                  ? "bg-slate-800/90 border-indigo-500/80 shadow-lg shadow-indigo-950/40 text-slate-100"
                  : "bg-surface/60 hover:bg-surface border-border/70 text-slate-300"
              }`}
            >
              {isActive && (
                <div className="absolute left-0 top-3 bottom-3 w-1 bg-indigo-500 rounded-r"></div>
              )}

              <div className="flex items-start justify-between gap-2">
                <div className="flex items-center gap-1.5">
                  <span className="font-mono text-xs font-bold text-indigo-400 bg-indigo-950/60 px-1.5 py-0.5 rounded border border-indigo-800/50">
                    {plan.meta.number || plan.id}
                  </span>
                  <h3 className="text-xs font-semibold truncate max-w-[140px] text-slate-200">
                    {plan.meta.topic || plan.title}
                  </h3>
                </div>

                {isComplete ? (
                  <CheckCircle className="w-3.5 h-3.5 text-emerald-400 shrink-0" />
                ) : plan.awaiting ? (
                  <AlertTriangle className="w-3.5 h-3.5 text-amber-400 shrink-0 animate-bounce" />
                ) : (
                  <Clock className="w-3.5 h-3.5 text-slate-500 shrink-0" />
                )}
              </div>

              <p className="text-[11px] text-slate-400 line-clamp-1 mt-1 font-normal">
                {plan.goal}
              </p>

              {/* Progress bars */}
              <div className="mt-2.5 space-y-1">
                <div className="flex justify-between text-[10px] font-mono text-slate-400">
                  <span>Built {stats.builtPercent}%</span>
                  <span>Proven {stats.provenPercent}%</span>
                </div>
                <div className="w-full h-1.5 bg-slate-900 rounded-full overflow-hidden flex">
                  <div
                    className="h-full bg-cyan-500 transition-all duration-300"
                    style={{ width: `${stats.builtPercent}%` }}
                  ></div>
                  <div
                    className="h-full bg-emerald-500 transition-all duration-300"
                    style={{ width: `${stats.provenPercent}%` }}
                  ></div>
                </div>
              </div>

              {/* Branch / Model Footprint */}
              <div className="mt-2 pt-2 border-t border-slate-700/40 flex items-center justify-between text-[10px] font-mono text-slate-400">
                <span className="truncate max-w-[110px] text-slate-400">{plan.meta.branch}</span>
                <span className="text-indigo-400">{plan.meta.author?.model || "AI Agent"}</span>
              </div>
            </div>
          );
        })}
      </div>

      {/* Watched Directory Bar */}
      <div className="p-3 border-t border-[#30363d] bg-[#161b22]/50 text-xs">
        <div className="flex items-center gap-2 text-slate-400">
          <Folder className="w-3.5 h-3.5 text-indigo-400 shrink-0" />
          <div className="truncate">
            <div className="text-[10px] text-slate-500 uppercase font-semibold">Watching Directory</div>
            <div className="font-mono text-[11px] text-slate-300 truncate" title={watchPath}>
              {watchPath}
            </div>
          </div>
        </div>
      </div>
    </aside>
  );
};
