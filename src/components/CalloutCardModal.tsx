import React from "react";
import { Plan } from "../types/plan";
import { PlanService } from "../services/planService";
import { X, CheckCircle, ShieldCheck, ExternalLink } from "lucide-react";

interface CalloutCardModalProps {
  plan: Plan;
  isOpen: boolean;
  onClose: () => void;
  onApproveSpine: () => void;
}

export const CalloutCardModal: React.FC<CalloutCardModalProps> = ({
  plan,
  isOpen,
  onClose,
  onApproveSpine
}) => {
  if (!isOpen) return null;

  const stats = PlanService.calculateStats(plan);
  const numberStr = (plan.meta.number || plan.id).padEnd(8, " ");
  const boardUrl = "http://localhost:5180".padEnd(48, " ");
  const stateStr = `${stats.builtItems}/${stats.totalItems} built  ${stats.provenItems}/${stats.totalItems} proven  ${stats.integrityScore}% ok`.padEnd(48, " ");
  const needStr = (plan.approved === "pending" 
    ? "approve the spine - click APPROVE on the board"
    : plan.awaiting?.note || "all gates clear - ready for execution"
  ).padEnd(48, " ");

  const asciiCard = `╔══════════════════════════════════════════════════════════════╗
║  CHECK THE PERFECT PLANNER                        ${numberStr}   ║
╠══════════════════════════════════════════════════════════════╣
║   ┌──────────────────────────────────────────────────────┐   ║
║   │                   _.-"""""""-._                      │   ║
║   │                .-'             '-.                   │   ║
║   │                /    _.-"""-._    \\                   │   ║
║   │               |   .'         '.   |                  │   ║
║   │               |  |    .---.    |  |                  │   ║
║   │               |  |   ( ( ) )   |  |                  │   ║
║   │               |  |    '---'    |  |                  │   ║
║   │               |   '.         .'   |                  │   ║
║   │                \\    '-.....-'    /                   │   ║
║   │               '-._             _.-'                  │   ║
║   │                  '-.._______..-'                     │   ║
║   │                                                      │   ║
║   │                B y   l o o p l e t                   │   ║
║   │                                                      │   ║
║   └──────────────────────────────────────────────────────┘   ║
║                                                              ║
║   BOARD   ${boardUrl}   ║
║   STATE   ${stateStr}   ║
║   NEED    ${needStr}   ║
╚══════════════════════════════════════════════════════════════╝`;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/80 backdrop-blur-sm">
      <div className="bg-[#0f141c] border border-slate-700 rounded-2xl max-w-2xl w-full shadow-2xl overflow-hidden animate-in fade-in zoom-in-95 duration-150">
        {/* Modal Header */}
        <div className="px-6 py-4 border-b border-slate-800 flex items-center justify-between bg-[#161b22]">
          <div className="flex items-center gap-2 text-indigo-400 font-mono font-bold text-sm">
            <ShieldCheck className="w-4 h-4" />
            <span>Perfect Planner Callout Frame (64x25)</span>
          </div>
          <button onClick={onClose} className="text-slate-400 hover:text-slate-200">
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* ASCII Card Presentation */}
        <div className="p-6 overflow-x-auto flex justify-center">
          <pre className="font-mono text-[11px] leading-[14px] text-indigo-200 bg-[#070a0f] p-4 rounded-xl border border-indigo-950/80 shadow-inner select-all">
            {asciiCard}
          </pre>
        </div>

        {/* Modal Actions */}
        <div className="px-6 py-4 bg-[#161b22] border-t border-slate-800 flex items-center justify-between">
          <span className="text-xs text-slate-400 font-mono">
            Status: <span className="text-emerald-400 font-bold">{plan.approved.toUpperCase()}</span>
          </span>

          <div className="flex items-center gap-3">
            {plan.approved !== "yes" && (
              <button
                onClick={() => {
                  onApproveSpine();
                  onClose();
                }}
                className="px-4 py-2 text-xs font-bold text-slate-900 bg-amber-400 hover:bg-amber-300 rounded-xl transition flex items-center gap-2"
              >
                <CheckCircle className="w-4 h-4" />
                <span>Approve Spine Now</span>
              </button>
            )}
            <button
              onClick={onClose}
              className="px-4 py-2 text-xs font-medium text-slate-300 bg-slate-800 hover:bg-slate-700 rounded-xl transition"
            >
              Dismiss
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
