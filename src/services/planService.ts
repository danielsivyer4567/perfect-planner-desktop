import { Plan, Vertebra, ChecklistItem, ChecklistProof } from "../types/plan";
import { SAMPLE_PLANS } from "./samplePlans";

const STORAGE_KEY = "perfect_planner_plans_v1";

export class PlanService {
  private static loadStoredPlans(): Plan[] {
    try {
      const data = localStorage.getItem(STORAGE_KEY);
      if (data) {
        return JSON.parse(data);
      }
    } catch (e) {
      console.warn("Failed to load plans from localStorage", e);
    }
    return SAMPLE_PLANS;
  }

  private static saveStoredPlans(plans: Plan[]): void {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(plans));
    } catch (e) {
      console.warn("Failed to persist plans to localStorage", e);
    }
  }

  public static async getPlans(): Promise<Plan[]> {
    // Check if running inside Tauri
    if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const tauriPlans = await invoke<Plan[]>("get_plans");
        if (tauriPlans && tauriPlans.length > 0) {
          return tauriPlans;
        }
      } catch (e) {
        console.warn("Tauri get_plans IPC fallback to local storage:", e);
      }
    }

    // Try scanning localhost:5230 if running
    try {
      const resp = await fetch("http://localhost:5230/plan", { signal: AbortSignal.timeout(600) });
      if (resp.ok) {
        const livePlan = await resp.json();
        const stored = this.loadStoredPlans();
        const existingIdx = stored.findIndex(p => p.id === (livePlan.meta?.number || livePlan.id));
        if (existingIdx >= 0) {
          stored[existingIdx] = { ...stored[existingIdx], ...livePlan, id: livePlan.meta?.number || livePlan.id };
        } else {
          stored.unshift({ ...livePlan, id: livePlan.meta?.number || "PP-LIVE" });
        }
        this.saveStoredPlans(stored);
        return stored;
      }
    } catch {
      // 5230 not active, continue with stored plans
    }

    return this.loadStoredPlans();
  }

  public static savePlan(plan: Plan): void {
    const plans = this.loadStoredPlans();
    const idx = plans.findIndex(p => p.id === plan.id);
    if (idx >= 0) {
      plans[idx] = plan;
    } else {
      plans.unshift(plan);
    }
    this.saveStoredPlans(plans);
  }

  public static toggleItem(
    planId: string,
    vertebraId: string,
    itemIndex: number,
    field: "built" | "tested"
  ): Plan | null {
    const plans = this.loadStoredPlans();
    const plan = plans.find(p => p.id === planId);
    if (!plan) return null;

    const vert = plan.vertebrae.find(v => v.id === vertebraId);
    if (!vert || !vert.checklist[itemIndex]) return null;

    const item = vert.checklist[itemIndex];
    const newVal = !item[field];
    item[field] = newVal;

    // If tested is toggled on, automatically stamp proof
    if (field === "tested" && newVal && !item.proof) {
      item.proof = {
        by: "user",
        at: new Date().toISOString(),
        note: `Confirmed by user in Perfect Planner Desktop`,
        verify: item.verify || "manual-review",
        git: { sha: plan.baselineSha || "HEAD", branch: plan.meta.branch, dirty: false }
      };
    } else if (field === "tested" && !newVal) {
      // If unchecked tested, clear proof
      item.proof = undefined;
    }

    // Check if vertebra is complete
    const allTested = vert.checklist.every(c => c.tested);
    const anyInProgress = vert.checklist.some(c => c.built || c.tested);
    if (allTested) {
      vert.status = "complete";
    } else if (anyInProgress) {
      vert.status = "in-progress";
    } else {
      vert.status = "pending";
    }

    this.savePlan(plan);
    return plan;
  }

  public static calculateStats(plan: Plan) {
    let totalItems = 0;
    let builtItems = 0;
    let testedItems = 0;
    let provenItems = 0;

    plan.vertebrae.forEach(v => {
      v.checklist.forEach(item => {
        totalItems++;
        if (item.built) builtItems++;
        if (item.tested) testedItems++;
        if (item.proof) provenItems++;
      });
    });

    const builtPercent = totalItems > 0 ? Math.round((builtItems / totalItems) * 100) : 0;
    const testedPercent = totalItems > 0 ? Math.round((testedItems / totalItems) * 100) : 0;
    const provenPercent = totalItems > 0 ? Math.round((provenItems / totalItems) * 100) : 0;

    const isSpineApproved = plan.approved === "yes";
    const integrityScore = Math.round((builtPercent * 0.4) + (provenPercent * 0.6));

    return {
      totalItems,
      builtItems,
      testedItems,
      provenItems,
      builtPercent,
      testedPercent,
      provenPercent,
      isSpineApproved,
      integrityScore
    };
  }

  public static generateMarkdownExport(plan: Plan): string {
    const stats = this.calculateStats(plan);
    let md = `# ${plan.meta.number} • ${plan.title}\n\n`;
    md += `> **Project:** ${plan.meta.project} | **Branch:** \`${plan.meta.branch}\` | **Approved:** ${plan.approved.toUpperCase()}\n`;
    md += `> **Stats:** ${stats.builtItems}/${stats.totalItems} built (${stats.builtPercent}%), ${stats.provenItems}/${stats.totalItems} proven (${stats.provenPercent}%)\n\n`;
    md += `## Goal\n${plan.goal}\n\n`;

    md += `## Spine Structure\n`;
    plan.spine.forEach((p, idx) => {
      md += `${idx + 1}. **${p.title}** (${p.id}): ${p.summary}\n`;
    });
    md += `\n---\n\n## Vertebrae & Checklist\n\n`;

    plan.vertebrae.forEach(v => {
      md += `### [${v.id}] ${v.title} (${v.status.toUpperCase()})\n`;
      if (v.files && v.files.length > 0) {
        md += `*Files:* \`${v.files.join("`, `")}\`\n\n`;
      }
      v.checklist.forEach(c => {
        const b = c.built ? "[x]" : "[ ]";
        const t = c.tested ? "[x]" : "[ ]";
        let line = `- Built ${b} Tested ${t} ${c.text}`;
        if (c.proof) {
          line += `\n  - *Proof (${c.proof.by}):* ${c.proof.note || "Verified"}`;
          if (c.proof.verify) line += ` (\`${c.proof.verify}\`)`;
        }
        md += line + "\n";
      });
      md += "\n";
    });

    return md;
  }
}
