import React, { useState } from "react";
import { Plan } from "../types/plan";
import { X, Sparkles, Plus } from "lucide-react";

interface NewPlanModalProps {
  isOpen: boolean;
  onClose: () => void;
  onCreate: (plan: Plan) => void;
}

export const NewPlanModal: React.FC<NewPlanModalProps> = ({
  isOpen,
  onClose,
  onCreate
}) => {
  const [number, setNumber] = useState("PP-004");
  const [topic, setTopic] = useState("");
  const [goal, setGoal] = useState("");
  const [project, setProject] = useState("Looplet");
  const [branch, setBranch] = useState("feat/new-plan");

  if (!isOpen) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!topic.trim()) return;

    const newPlan: Plan = {
      id: number,
      title: `${topic} Feature Plan`,
      goal: goal || `Implement and prove the ${topic} feature according to the spinal plan.`,
      createdAt: new Date().toISOString(),
      approved: "pending",
      meta: {
        number,
        topic,
        project,
        branch,
        appUrl: "http://localhost:5180",
        author: {
          model: "antigravity",
          user: "danie",
          at: new Date().toISOString()
        }
      },
      spine: [
        { id: "P1", title: "Foundation & Data Model", summary: "Types, migrations, and core dependencies." },
        { id: "P2", title: "Core Business Implementation", summary: "Components, workflows, and core functionality." },
        { id: "P3", title: "Testing, Proof & Verification", summary: "Automated test suites, UI verification, and signoff." }
      ],
      vertebrae: [
        {
          id: "V01",
          spineId: "P1",
          side: "L",
          title: "Schema & Interface Definitions",
          status: "pending",
          est: { tokens: 10000, minutes: 15 },
          checklist: [
            { text: `Define TypeScript types for ${topic}`, built: false, tested: false, verify: "npx tsc --noEmit" },
            { text: `Scaffold initial component or service`, built: false, tested: false }
          ]
        },
        {
          id: "V02",
          spineId: "P2",
          side: "R",
          title: "Core Feature Logic",
          status: "pending",
          est: { tokens: 20000, minutes: 30 },
          checklist: [
            { text: `Implement ${topic} user interface and handlers`, built: false, tested: false, ui: true },
            { text: `Integrate with data store and state management`, built: false, tested: false }
          ]
        }
      ]
    };

    onCreate(newPlan);
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/80 backdrop-blur-sm">
      <div className="bg-[#0f141c] border border-slate-700 rounded-2xl max-w-lg w-full shadow-2xl overflow-hidden animate-in fade-in zoom-in-95 duration-150">
        <div className="px-6 py-4 border-b border-slate-800 flex items-center justify-between bg-[#161b22]">
          <div className="flex items-center gap-2 text-indigo-400 font-mono font-bold text-sm">
            <Sparkles className="w-4 h-4" />
            <span>Create New Spinal Plan (/perfect-planning)</span>
          </div>
          <button onClick={onClose} className="text-slate-400 hover:text-slate-200">
            <X className="w-5 h-5" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="p-6 space-y-4">
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-xs font-semibold text-slate-400 uppercase tracking-wider mb-1">
                Plan Number
              </label>
              <input
                type="text"
                value={number}
                onChange={e => setNumber(e.target.value)}
                className="w-full bg-slate-900 border border-slate-700 rounded-xl px-3 py-2 text-xs font-mono text-slate-100 focus:outline-none focus:border-indigo-500"
                placeholder="PP-004"
                required
              />
            </div>
            <div>
              <label className="block text-xs font-semibold text-slate-400 uppercase tracking-wider mb-1">
                Project Name
              </label>
              <input
                type="text"
                value={project}
                onChange={e => setProject(e.target.value)}
                className="w-full bg-slate-900 border border-slate-700 rounded-xl px-3 py-2 text-xs text-slate-100 focus:outline-none focus:border-indigo-500"
                placeholder="Looplet"
              />
            </div>
          </div>

          <div>
            <label className="block text-xs font-semibold text-slate-400 uppercase tracking-wider mb-1">
              Topic / Feature Name
            </label>
            <input
              type="text"
              value={topic}
              onChange={e => setTopic(e.target.value)}
              className="w-full bg-slate-900 border border-slate-700 rounded-xl px-3 py-2 text-xs text-slate-100 focus:outline-none focus:border-indigo-500"
              placeholder="e.g. Automated Quoting, Invoicing Sync, Push Notifications"
              required
            />
          </div>

          <div>
            <label className="block text-xs font-semibold text-slate-400 uppercase tracking-wider mb-1">
              Branch Name
            </label>
            <input
              type="text"
              value={branch}
              onChange={e => setBranch(e.target.value)}
              className="w-full bg-slate-900 border border-slate-700 rounded-xl px-3 py-2 text-xs font-mono text-slate-100 focus:outline-none focus:border-indigo-500"
              placeholder="feat/my-topic"
            />
          </div>

          <div>
            <label className="block text-xs font-semibold text-slate-400 uppercase tracking-wider mb-1">
              Goal / Statement
            </label>
            <textarea
              value={goal}
              onChange={e => setGoal(e.target.value)}
              rows={2}
              className="w-full bg-slate-900 border border-slate-700 rounded-xl px-3 py-2 text-xs text-slate-100 focus:outline-none focus:border-indigo-500"
              placeholder="One-line statement of the end result this whole plan builds toward."
            />
          </div>

          <div className="pt-2 flex justify-end gap-2">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 text-xs font-medium text-slate-300 bg-slate-800 hover:bg-slate-700 rounded-xl transition"
            >
              Cancel
            </button>
            <button
              type="submit"
              className="px-4 py-2 text-xs font-bold text-white bg-indigo-600 hover:bg-indigo-500 rounded-xl transition flex items-center gap-1.5 shadow-md shadow-indigo-500/20"
            >
              <Plus className="w-4 h-4" />
              <span>Create Spinal Plan</span>
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
