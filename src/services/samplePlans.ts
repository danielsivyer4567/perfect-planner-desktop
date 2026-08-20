import { Plan } from "../types/plan";

export const SAMPLE_PLANS: Plan[] = [
  {
    id: "PP-003",
    filePath: "C:\\repos\\plans\\PP-003-tauri-planner-desktop.plan.json",
    title: "Tauri Planner Shell - Replace Browser as Plan-Board Container",
    goal: "Provide a dedicated, ultra-lightweight desktop and localhost container for Perfect Planning boards with auto-discovery, live-reload, and proof verification.",
    createdAt: "2026-08-20T23:30:00+10:00",
    baselineSha: "9f84b12",
    approved: "yes",
    approvalDate: "2026-08-20",
    meta: {
      number: "PP-003",
      project: "Perfect Planner Desktop",
      branch: "main",
      topic: "Tauri Desktop Shell",
      focus: "Lightweight container, folder watcher, multi-plan tabs, live reload, proof viewer",
      appUrl: "http://localhost:5180",
      author: {
        model: "gemini-3.7-flash",
        user: "danie",
        at: "2026-08-21T02:50:00+10:00"
      }
    },
    awaiting: null,
    ci: {
      derivedFrom: "package.json",
      workflowSha: "a1b2c3d",
      commands: [
        { id: "lint", job: "Lint", tier: 1, command: "npm run lint", blocking: true, measuredMs: 3200 },
        { id: "typecheck", job: "Typecheck", tier: 1, command: "tsc --noEmit", blocking: true, measuredMs: 4100 },
        { id: "build", job: "Build", tier: 2, command: "npm run build", blocking: true, measuredMs: 5800 }
      ],
      runs: [
        {
          runId: "ci-t1-local",
          tier: 1,
          at: "2026-08-21T02:52:00+10:00",
          ok: true,
          git: { head: "9f84b12", base: "origin/main" },
          results: [
            { id: "typecheck", exit: 0, ms: 4100, log: "typecheck-pass.log" },
            { id: "build", exit: 0, ms: 5800, log: "build-pass.log" }
          ]
        }
      ]
    },
    spine: [
      { id: "P1", title: "Foundation & Toolchain", summary: "Vite + React + Tailwind + Tauri v2 scaffolding and directory watchers." },
      { id: "P2", title: "Core Plan Visualizer", summary: "Spine & Vertebrae board, tabs, dual Built/Tested checklist, and Proof inspector." },
      { id: "P3", title: "Live Auto-Discovery & Hardening", summary: "Background folder watcher on C:\\repos\\plans, proof hashing, export bundles, and desktop packaging." }
    ],
    vertebrae: [
      {
        id: "V01",
        spineId: "P1",
        side: "L",
        title: "Project Scaffolding & Toolchain",
        status: "complete",
        est: { tokens: 12000, minutes: 15 },
        files: ["package.json", "vite.config.ts", "tailwind.config.js", "tsconfig.json"],
        checklist: [
          {
            text: "Initialize package.json with scripts for dev, build, preview and tauri",
            built: true,
            tested: true,
            verify: "npm run dev",
            ref: "package.json",
            builtBy: {
              model: "gemini-3.7-flash",
              user: "danie",
              at: "2026-08-21T02:54:00+10:00",
              git: { sha: "9f84b12", branch: "main", dirty: false }
            },
            proof: {
              by: "prove",
              at: "2026-08-21T02:54:10+10:00",
              note: "package.json successfully configured and verified",
              verify: "npm --version",
              git: { sha: "9f84b12", branch: "main", dirty: false }
            }
          },
          {
            text: "Configure Tailwind CSS with modern dark UI styling tokens",
            built: true,
            tested: true,
            verify: "npx tailwindcss -i ./src/index.css -o ./dist/output.css",
            ref: "tailwind.config.js",
            builtBy: {
              model: "gemini-3.7-flash",
              user: "danie",
              at: "2026-08-21T02:54:40+10:00"
            },
            proof: {
              by: "prove",
              at: "2026-08-21T02:54:50+10:00",
              note: "Tailwind and PostCSS verified"
            }
          }
        ]
      },
      {
        id: "V02",
        spineId: "P1",
        side: "R",
        title: "Tauri v2 Rust Backend & Watcher",
        status: "in-progress",
        est: { tokens: 18000, minutes: 25 },
        files: ["src-tauri/Cargo.toml", "src-tauri/tauri.conf.json", "src-tauri/src/main.rs"],
        checklist: [
          {
            text: "Configure Tauri v2 manifest with restricted allowlist for C:\\repos\\plans",
            built: true,
            tested: true,
            verify: "cargo check",
            ref: "src-tauri/tauri.conf.json",
            builtBy: { model: "gemini-3.7-flash", user: "danie", at: "2026-08-21T02:56:00+10:00" },
            proof: { by: "prove", at: "2026-08-21T02:56:10+10:00", note: "Allowlist scoped strictly to plan directory" }
          },
          {
            text: "Implement Tauri command get_plans and watch_plans for live file changes",
            built: true,
            tested: false,
            verify: "cargo test",
            ref: "src-tauri/src/lib.rs"
          }
        ]
      },
      {
        id: "V03",
        spineId: "P2",
        side: "L",
        title: "Spine & Vertebrae Visualizer UI",
        status: "in-progress",
        est: { tokens: 25000, minutes: 30 },
        files: ["src/components/SpineBoard.tsx", "src/components/VertebraCard.tsx"],
        checklist: [
          {
            text: "Render central Spine Phases with progressive built/tested progress indicators",
            built: true,
            tested: true,
            ui: true,
            verify: "npm run dev",
            proof: { by: "user", at: "2026-08-21T02:58:00+10:00", note: "Visual confirmation on http://localhost:5180" }
          },
          {
            text: "Render branching Vertebra cards with dual Built/Tested checklists and Proof badges",
            built: true,
            tested: true,
            ui: true,
            verify: "npm run dev",
            proof: { by: "user", at: "2026-08-21T02:58:20+10:00", note: "Interactive toggle and proof drawers verified" }
          },
          {
            text: "Implement Proof modal showing machine-captured logs, token estimates, and git SHAs",
            built: true,
            tested: true,
            ui: true,
            verify: "npm run dev"
          }
        ]
      },
      {
        id: "V04",
        spineId: "P2",
        side: "R",
        title: "Multi-Plan Tab Manager & Header",
        status: "complete",
        est: { tokens: 15000, minutes: 20 },
        files: ["src/components/PlanSidebar.tsx", "src/components/PlanHeader.tsx"],
        checklist: [
          {
            text: "Sidebar showing all active plans with health score, stats, and active branch",
            built: true,
            tested: true,
            ui: true,
            proof: { by: "user", at: "2026-08-21T02:58:30+10:00", note: "Fast tab switching tested" }
          },
          {
            text: "Fixed-frame 'CHECK THE PERFECT PLANNER' callout drawer with 64x25 ASCII mark",
            built: true,
            tested: true,
            ui: true,
            proof: { by: "user", at: "2026-08-21T02:58:40+10:00", note: "ASCII frame rendered cleanly" }
          }
        ]
      },
      {
        id: "V05",
        spineId: "P3",
        side: "L",
        title: "Live Auto-Discovery & Export",
        status: "in-progress",
        est: { tokens: 20000, minutes: 25 },
        files: ["src/services/planService.ts", "src/components/ExportModal.tsx"],
        checklist: [
          {
            text: "Export self-contained Markdown checklist (/export.md) with proof lines",
            built: true,
            tested: true,
            verify: "npm run test:export",
            proof: { by: "prove", at: "2026-08-21T02:58:50+10:00", note: "Markdown export generated with full proof trail" }
          },
          {
            text: "Auto-scan running perfect-plan-server.cjs instances on ports 5230-5240",
            built: true,
            tested: false,
            verify: "curl http://localhost:5230/plan"
          }
        ]
      }
    ]
  },
  {
    id: "PP-001",
    filePath: "C:\\repos\\plans\\PP-001-invoicing.plan.json",
    title: "Invoicing Feature & Payment Reconciliation",
    goal: "Build full invoice lifecycle from quote generation, deposit collection, Xero sync, and PDF invoice receipting.",
    createdAt: "2026-08-14T09:00:00+10:00",
    baselineSha: "63745ab963",
    approved: "yes",
    approvalDate: "2026-08-14",
    meta: {
      number: "PP-001",
      project: "Looplet CRM",
      branch: "codex/ui-invoicing-clean",
      topic: "Invoicing",
      focus: "the invoice creation flow - data model, form, PDF export, and edge-case safety",
      appUrl: "http://localhost:5173",
      author: {
        model: "claude-fable-5",
        user: "danie",
        at: "2026-08-14T09:00:00+10:00"
      }
    },
    awaiting: null,
    spine: [
      { id: "P1", title: "Data Model & Migrations", summary: "Invoices table, line items, and RLS policies." },
      { id: "P2", title: "Invoice UI & PDF Generation", summary: "Wizard, preview modal, PDF export." },
      { id: "P3", title: "Accounting & Payment Webhooks", summary: "Xero synchronization and Stripe webhook reconciliation." }
    ],
    vertebrae: [
      {
        id: "V01",
        spineId: "P1",
        side: "L",
        title: "Invoices Database Schema",
        status: "complete",
        est: { tokens: 15000, minutes: 20 },
        files: ["src/types/invoice.ts", "supabase/migrations/20260814090000_invoices.sql"],
        checklist: [
          {
            text: "Define TypeScript Invoice and InvoiceLineItem interfaces",
            built: true,
            tested: true,
            verify: "npx tsc --noEmit",
            proof: { by: "prove", at: "2026-08-14T09:30:00+10:00", note: "TypeScript type check passed" }
          },
          {
            text: "Add Supabase migration with RLS org membership check",
            built: true,
            tested: true,
            verify: "supabase db test",
            proof: { by: "prove", at: "2026-08-14T10:15:00+10:00", note: "RLS policy verified" }
          }
        ]
      }
    ]
  }
];
