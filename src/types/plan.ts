export interface PlanAuthor {
  model: string;
  user: string;
  at: string;
}

export interface PlanMeta {
  number: string;
  project: string;
  branch: string;
  topic: string;
  focus?: string;
  appUrl?: string;
  author?: PlanAuthor;
}

export interface SpinePhase {
  id: string;
  title: string;
  summary: string;
}

export interface ChecklistGit {
  sha: string;
  branch: string;
  dirty: boolean;
}

export interface ChecklistLocFile {
  file: string;
  added: number;
  removed: number;
}

export interface ChecklistLoc {
  baseline?: string;
  scope?: string;
  added: number;
  removed: number;
  dirty?: boolean;
  files?: ChecklistLocFile[];
}

export interface ChecklistBuiltBy {
  model: string;
  user: string;
  session?: string;
  at: string;
  git?: ChecklistGit;
  tokensEst?: number;
  loc?: ChecklistLoc;
}

export interface ChecklistProof {
  by: string;
  at: string;
  model?: string;
  user?: string;
  note?: string;
  log?: string;
  sha256?: string;
  git?: ChecklistGit;
  verify?: string;
  tokensEst?: number;
  loc?: ChecklistLoc;
  screenshot?: string;
}

export interface ChecklistItem {
  id?: string;
  text: string;
  built: boolean;
  tested: boolean;
  verify?: string;
  ref?: string;
  ui?: boolean;
  needs?: string;
  builtBy?: ChecklistBuiltBy;
  proof?: ChecklistProof;
}

export interface Gate {
  type: string;
  withPlan?: string;
  withVertebra?: string;
  files?: string[];
  resources?: string[];
  notedAt: string;
  status: "cleared" | "open" | "blocked";
  clearedAt?: string;
  clearNote?: string;
}

export interface VertebraRef {
  source: string;
  file: string;
  lines?: string;
  label?: string;
  snippet?: string;
}

export interface VertebraEst {
  tokens?: number;
  minutes?: number;
}

export interface Vertebra {
  id: string;
  spineId: string;
  side: "L" | "R";
  title: string;
  status: "in-progress" | "complete" | "pending" | "blocked";
  dependsOn?: string[];
  est?: VertebraEst;
  files?: string[];
  resources?: string[];
  startSha?: string;
  lastStampSha?: string;
  owner?: string;
  scoreExempt?: boolean;
  docs?: string[];
  gates?: Gate[];
  notes?: string;
  refs?: VertebraRef[];
  checklist: ChecklistItem[];
}

export interface CiCommand {
  id: string;
  job: string;
  tier: number;
  command: string;
  scoped?: string;
  blocking?: boolean;
  measuredMs?: number;
}

export interface CiRunResult {
  id: string;
  exit: number;
  ms: number;
  log: string;
}

export interface CiRun {
  runId: string;
  tier: number;
  at: string;
  ok: boolean;
  git: {
    head: string;
    base: string;
    merge?: string;
    conflicts?: string[];
  };
  results: CiRunResult[];
}

export interface CiConfig {
  derivedFrom?: string;
  workflowSha?: string;
  node?: string;
  install?: string;
  env?: Record<string, string>;
  commands: CiCommand[];
  nonBlocking?: Array<{ name: string; reason: string }>;
  runs?: CiRun[];
}

export interface AwaitingState {
  kind: "confirm" | "approve" | "unblock" | "input";
  item?: string;
  since: string;
  note: string;
  pid?: number;
}

export interface Plan {
  id: string;
  filePath?: string;
  title: string;
  goal: string;
  createdAt: string;
  baselineSha?: string;
  approved: "yes" | "pending" | "no";
  approvalDate?: string;
  meta: PlanMeta;
  protocol?: {
    version: number;
    note?: string;
    rules?: string[] | null;
  };
  awaiting?: AwaitingState | null;
  ci?: CiConfig;
  spine: SpinePhase[];
  vertebrae: Vertebra[];
  journal?: Array<{
    seq: number;
    at: string;
    action: string;
    item?: string;
    sha256?: string;
    prevSha256?: string;
  }>;
}
