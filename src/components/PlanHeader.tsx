import React from "react";
import { Plan } from "../types/plan";
import { PlanService } from "../services/planService";
import { CheckCircle2, ShieldCheck, FileText, Bell, RefreshCw, Plus, ExternalLink, Cpu } from "lucide-react";

interface PlanHeaderProps {
  plan: Plan;
  onOpenCallout: () => void;
  onOpenExport: () => void;
  onOpenNewPlan: () => void;
  onRefresh: () => void;
  isRefreshing: boolean;
}

export const PlanHeader: React.FC<PlanHeaderProps> = ({
  plan,
  onOpenCallout,
  onOpenExport,
  onOpenNewPlan,
  onRefresh,
  isRefreshing
}) => {
  const stats = PlanService.calculateStats(plan);
  const isApproved = plan.approved === "yes";

  return (
    <header className="border-b border-[#30363d] bg-[#161b22]/90 backdrop-blur px-6 py-3.5 flex flex-col gap-3">
      {/* Official Top Banner */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-2 bg-indigo-950/80 border border-indigo-700/60 px-3 py-1 rounded-md text-xs font-mono font-bold text-indigo-300">
            <Cpu className="w-3.5 h-3.5 text-indigo-400" />
            <span>{plan.meta.number}</span>
          </div>
          <span className="text-slate-500 font-bold">•</span>
          <span className="text-sm font-semibold text-slate-200">{plan.meta.topic}</span>
          <span className="text-slate-500 font-bold">•</span>
          <span className="text-xs text-slate-400 font-mono bg-slate-800/80 px-2 py-0.5 rounded border border-slate-700">
            {plan.meta.project} / {plan.meta.branch}
          </span>
          <span className="text-slate-500 font-bold">•</span>
          <div className="flex items-center gap-1.5 text-xs text-emerald-400 bg-emerald-950/40 border border-emerald-800/50 px-2 py-0.5 rounded-full font-medium">
            <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse"></span>
            <span>board live</span>
          </div>
        </div>

        {/* Action buttons */}
        <div className="flex items-center gap-2">
          <button
            onClick={onOpenCallout}
            className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-semibold text-amber-300 bg-amber-950/50 hover:bg-amber-900/60 border border-amber-600/60 rounded-lg transition shadow-sm"
          >
            <Bell className="w-3.5 h-3.5 text-amber-400" />
            <span>Check Planner</span>
            {plan.awaiting && (
              <span className="w-2 h-2 rounded-full bg-amber-400 animate-ping ml-0.5"></span>
            )}
          </button>

          <button
            onClick={onOpenExport}
            className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-slate-300 bg-slate-800 hover:bg-slate-700 border border-slate-600 rounded-lg transition"
          >
            <FileText className="w-3.5 h-3.5 text-slate-400" />
            <span>Export .md</span>
          </button>

          <button
            onClick={onOpenNewPlan}
            className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-white bg-indigo-600 hover:bg-indigo-500 rounded-lg transition shadow-sm shadow-indigo-500/20"
          >
            <Plus className="w-3.5 h-3.5" />
            <span>New Plan</span>
          </button>

          <button
            onClick={onRefresh}
            title="Scan & Live Reload"
            className="p-1.5 text-slate-400 hover:text-slate-200 bg-slate-800/80 hover:bg-slate-700 border border-slate-700 rounded-lg transition"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${isRefreshing ? "animate-spin text-indigo-400" : ""}`} />
          </button>
        </div>
      </div>

      {/* Plan Goal and Key Metric Chips */}
      <div className="flex items-center justify-between text-xs pt-1 border-t border-slate-800/80">
        <div className="text-slate-300 truncate max-w-2xl">
          <span className="text-slate-500 font-semibold uppercase tracking-wider mr-2">Goal:</span>
          {plan.goal}
        </div>

        <div className="flex items-center gap-4 text-xs font-mono">
          <div className="flex items-center gap-1.5">
            <span className="text-slate-400">Approval:</span>
            <span className={`px-2 py-0.5 rounded font-bold uppercase text-[10px] ${
              isApproved ? "bg-emerald-950 text-emerald-300 border border-emerald-700" : "bg-amber-950 text-amber-300 border border-amber-700"
            }`}>
              {plan.approved}
            </span>
          </div>

          <div className="flex items-center gap-1.5">
            <span className="text-slate-400">Built:</span>
            <span className="text-cyan-300 font-semibold">{stats.builtItems}/{stats.totalItems} ({stats.builtPercent}%)</span>
          </div>

          <div className="flex items-center gap-1.5">
            <span className="text-slate-400">Proven:</span>
            <span className="text-emerald-400 font-semibold">{stats.provenItems}/{stats.totalItems} ({stats.provenPercent}%)</span>
          </div>

          <div className="flex items-center gap-1.5 bg-slate-800/60 px-2 py-0.5 rounded border border-slate-700">
            <ShieldCheck className="w-3.5 h-3.5 text-indigo-400" />
            <span className="text-slate-300">Integrity:</span>
            <span className="text-indigo-300 font-bold">{stats.integrityScore}%</span>
          </div>
        </div>
      </div>
    </header>
  );
};
