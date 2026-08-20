import React from "react";
import { ChecklistItem } from "../types/plan";
import { X, Shield, GitCommit, Terminal, Clock, FileCode, CheckCircle2 } from "lucide-react";

interface ProofModalProps {
  item: ChecklistItem | null;
  vertebraTitle: string;
  isOpen: boolean;
  onClose: () => void;
}

export const ProofModal: React.FC<ProofModalProps> = ({
  item,
  vertebraTitle,
  isOpen,
  onClose
}) => {
  if (!isOpen || !item) return null;

  const proof = item.proof;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/80 backdrop-blur-sm">
      <div className="bg-[#0f141c] border border-slate-700 rounded-2xl max-w-2xl w-full shadow-2xl overflow-hidden animate-in fade-in zoom-in-95 duration-150">
        {/* Header */}
        <div className="px-6 py-4 border-b border-slate-800 flex items-center justify-between bg-[#161b22]">
          <div className="flex items-center gap-2 text-emerald-400 font-mono font-bold text-sm">
            <Shield className="w-4 h-4" />
            <span>Captured Machine Proof Verification</span>
          </div>
          <button onClick={onClose} className="text-slate-400 hover:text-slate-200">
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Content */}
        <div className="p-6 space-y-4">
          {/* Target Item */}
          <div className="p-3.5 rounded-xl bg-slate-900/80 border border-slate-800">
            <div className="text-[11px] font-mono text-indigo-400 font-bold uppercase">{vertebraTitle}</div>
            <div className="text-sm font-semibold text-slate-100 mt-1">{item.text}</div>
          </div>

          {proof ? (
            <div className="space-y-3 font-mono text-xs">
              {/* Note / Result */}
              <div className="p-3 rounded-xl bg-emerald-950/30 border border-emerald-800/40 text-emerald-300">
                <div className="text-[10px] text-emerald-500 font-bold uppercase mb-1 flex items-center gap-1.5">
                  <CheckCircle2 className="w-3.5 h-3.5" />
                  <span>Attestation Statement</span>
                </div>
                <div>{proof.note || "Automated verification completed with exit code 0"}</div>
              </div>

              {/* Execution details grid */}
              <div className="grid grid-cols-2 gap-3">
                <div className="p-3 rounded-xl bg-slate-900/60 border border-slate-800/80">
                  <div className="text-[10px] text-slate-500 uppercase flex items-center gap-1 mb-1">
                    <Terminal className="w-3 h-3 text-indigo-400" /> Verify Command
                  </div>
                  <div className="text-slate-200 truncate">{proof.verify || item.verify || "manual check"}</div>
                </div>

                <div className="p-3 rounded-xl bg-slate-900/60 border border-slate-800/80">
                  <div className="text-[10px] text-slate-500 uppercase flex items-center gap-1 mb-1">
                    <GitCommit className="w-3 h-3 text-cyan-400" /> Git Commit SHA
                  </div>
                  <div className="text-slate-200 truncate">{proof.git?.sha || "HEAD"} ({proof.git?.branch || "main"})</div>
                </div>

                <div className="p-3 rounded-xl bg-slate-900/60 border border-slate-800/80">
                  <div className="text-[10px] text-slate-500 uppercase flex items-center gap-1 mb-1">
                    <Clock className="w-3 h-3 text-amber-400" /> Timestamp
                  </div>
                  <div className="text-slate-200 truncate">{new Date(proof.at).toLocaleString()}</div>
                </div>

                <div className="p-3 rounded-xl bg-slate-900/60 border border-slate-800/80">
                  <div className="text-[10px] text-slate-500 uppercase flex items-center gap-1 mb-1">
                    <FileCode className="w-3 h-3 text-emerald-400" /> Proven By
                  </div>
                  <div className="text-slate-200">{proof.by} ({proof.model || "automated"})</div>
                </div>
              </div>

              {/* Artifact SHA256 */}
              {proof.sha256 && (
                <div className="p-3 rounded-xl bg-slate-900/40 border border-slate-800 text-[11px]">
                  <span className="text-slate-500 uppercase text-[10px] block mb-0.5">Append-Only SHA256 Hash Chain</span>
                  <span className="text-indigo-300 break-all">{proof.sha256}</span>
                </div>
              )}
            </div>
          ) : (
            <div className="p-6 text-center text-slate-400 text-xs">
              No machine proof has been recorded for this item yet.
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="px-6 py-4 bg-[#161b22] border-t border-slate-800 flex justify-end">
          <button
            onClick={onClose}
            className="px-4 py-2 text-xs font-medium text-slate-300 bg-slate-800 hover:bg-slate-700 rounded-xl transition"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
};
