import React, { useState, useEffect, useCallback } from "react";
import { Plan, ChecklistItem } from "./types/plan";
import { PlanService } from "./services/planService";
import { PlanHeader } from "./components/PlanHeader";
import { PlanSidebar } from "./components/PlanSidebar";
import { SpineBoard } from "./components/SpineBoard";
import { CalloutCardModal } from "./components/CalloutCardModal";
import { ProofModal } from "./components/ProofModal";
import { ExportModal } from "./components/ExportModal";
import { NewPlanModal } from "./components/NewPlanModal";

export const App: React.FC = () => {
  const [plans, setPlans] = useState<Plan[]>([]);
  const [activePlanId, setActivePlanId] = useState<string>("PP-003");
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [watchPath] = useState("C:\\repos\\plans");

  // Modals state
  const [isCalloutOpen, setIsCalloutOpen] = useState(false);
  const [isExportOpen, setIsExportOpen] = useState(false);
  const [isNewPlanOpen, setIsNewPlanOpen] = useState(false);
  const [selectedProof, setSelectedProof] = useState<{ item: ChecklistItem; vertebraTitle: string } | null>(null);

  const loadPlans = useCallback(async () => {
    setIsRefreshing(true);
    try {
      const fetched = await PlanService.getPlans();
      setPlans(fetched);
      if (fetched.length > 0 && !fetched.find(p => p.id === activePlanId)) {
        setActivePlanId(fetched[0].id);
      }
    } finally {
      setTimeout(() => setIsRefreshing(false), 300);
    }
  }, [activePlanId]);

  useEffect(() => {
    loadPlans();
    // Auto-polling interval for live sync
    const interval = setInterval(loadPlans, 4000);
    return () => clearInterval(interval);
  }, [loadPlans]);

  const activePlan = plans.find(p => p.id === activePlanId) || plans[0];

  const handleToggleItem = (vertebraId: string, itemIndex: number, field: "built" | "tested") => {
    if (!activePlan) return;
    const updated = PlanService.toggleItem(activePlan.id, vertebraId, itemIndex, field);
    if (updated) {
      setPlans(prev => prev.map(p => (p.id === updated.id ? { ...updated } : p)));
    }
  };

  const handleApproveSpine = () => {
    if (!activePlan) return;
    const updated: Plan = {
      ...activePlan,
      approved: "yes",
      approvalDate: new Date().toISOString().split("T")[0]
    };
    PlanService.savePlan(updated);
    setPlans(prev => prev.map(p => (p.id === updated.id ? updated : p)));
  };

  const handleCreatePlan = (newPlan: Plan) => {
    PlanService.savePlan(newPlan);
    setPlans(prev => [newPlan, ...prev]);
    setActivePlanId(newPlan.id);
  };

  if (!activePlan) {
    return (
      <div className="h-screen w-screen flex items-center justify-center bg-[#0b0f17] text-slate-400">
        <div className="text-center font-mono">
          <div className="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin mx-auto mb-3"></div>
          <p>Connecting to Perfect Planner Engine...</p>
        </div>
      </div>
    );
  }

  return (
    <div className="h-screen w-screen flex overflow-hidden bg-[#0b0f17]">
      {/* Sidebar with all plans */}
      <PlanSidebar
        plans={plans}
        activePlanId={activePlanId}
        onSelectPlan={setActivePlanId}
        watchPath={watchPath}
      />

      {/* Main Content Area */}
      <main className="flex-1 flex flex-col min-w-0 h-full overflow-hidden bg-[#0b0f17]">
        <PlanHeader
          plan={activePlan}
          onOpenCallout={() => setIsCalloutOpen(true)}
          onOpenExport={() => setIsExportOpen(true)}
          onOpenNewPlan={() => setIsNewPlanOpen(true)}
          onRefresh={loadPlans}
          isRefreshing={isRefreshing}
        />

        <SpineBoard
          plan={activePlan}
          onToggleItem={handleToggleItem}
          onViewProof={(item, title) => setSelectedProof({ item, vertebraTitle: title })}
          onApproveSpine={handleApproveSpine}
        />
      </main>

      {/* Modals */}
      <CalloutCardModal
        plan={activePlan}
        isOpen={isCalloutOpen}
        onClose={() => setIsCalloutOpen(false)}
        onApproveSpine={handleApproveSpine}
      />

      <ProofModal
        item={selectedProof?.item || null}
        vertebraTitle={selectedProof?.vertebraTitle || ""}
        isOpen={!!selectedProof}
        onClose={() => setSelectedProof(null)}
      />

      <ExportModal
        plan={activePlan}
        isOpen={isExportOpen}
        onClose={() => setIsExportOpen(false)}
      />

      <NewPlanModal
        isOpen={isNewPlanOpen}
        onClose={() => setIsNewPlanOpen(false)}
        onCreate={handleCreatePlan}
      />
    </div>
  );
};
