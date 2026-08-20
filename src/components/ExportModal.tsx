import React, { useState } from "react";
import { Plan } from "../types/plan";
import { PlanService } from "../services/planService";
import { X, Copy, Check, FileDown } from "lucide-react";

interface ExportModalProps {
  plan: Plan;
  isOpen: boolean;
  onClose: () => void;
}

export const ExportModal: React.FC<ExportModalProps> = ({
  plan,
  isOpen,
  onClose
}) => {
  const [copied, setCopied] = useState(false);
  if (!isOpen) return null;

  const markdown = PlanService.generateMarkdownExport(plan);

  const handleCopy = () => {
    navigator.clipboard.writeText(markdown);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/80 backdrop-blur-sm">
      <div className="bg-[#0f141c] border border-slate-700 rounded-2xl max-w-3xl w-full max-h-[85vh] shadow-2xl flex flex-col overflow-hidden animate-in fade-in zoom-in-95 duration-150">
        {/* Header */}
        <div className="px-6 py-4 border-b border-slate-800 flex items-center justify-between bg-[#161b22]">
          <div className="flex items-center gap-2 text-indigo-400 font-mono font-bold text-sm">
            <FileDown className="w-4 h-4" />
            <span>Export Plan Markdown (/export.md)</span>
          </div>
          <button onClick={onClose} className="text-slate-400 hover:text-slate-200">
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Markdown Content */}
        <div className="flex-1 overflow-y-auto p-6 bg-[#070a0f]">
          <pre className="font-mono text-xs text-slate-300 whitespace-pre-wrap leading-relaxed select-all">
            {markdown}
          </pre>
        </div>

        {/* Footer */}
        <div className="px-6 py-4 bg-[#161b22] border-t border-slate-800 flex items-center justify-between">
          <span className="text-xs text-slate-400 font-mono">
            {plan.meta.number} • {plan.meta.topic}
          </span>
          <div className="flex items-center gap-2">
            <button
              onClick={handleCopy}
              className="flex items-center gap-1.5 px-4 py-2 text-xs font-semibold text-slate-900 bg-indigo-400 hover:bg-indigo-300 rounded-xl transition shadow-md shadow-indigo-500/20"
            >
              {copied ? <Check className="w-3.5 h-3.5 text-slate-900" /> : <Copy className="w-3.5 h-3.5 text-slate-900" />}
              <span>{copied ? "Copied to Clipboard" : "Copy Markdown"}</span>
            </button>
            <button
              onClick={onClose}
              className="px-4 py-2 text-xs font-medium text-slate-300 bg-slate-800 hover:bg-slate-700 rounded-xl transition"
            >
              Close
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
