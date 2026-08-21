export interface PlanMeta {
  number?: string;
  project?: string;
  branch?: string;
  topic?: string;
  focus?: string;
  appUrl?: string;
}

export interface SpinePhase {
  id: string;
  title: string;
  summary?: string;
}

export interface ProofGit {
  sha?: string;
  branch?: string;
  dirty?: boolean;
}

export interface ChecklistProof {
  by?: string;
  at?: string;
  model?: string;
  user?: string;
  session?: string;
  note?: string;
  log?: string;
  sha256?: string;
  screenshot?: string;
  screenshotSha256?: string;
  shotNote?: string;
  screenshotCheck?: { ok?: boolean; width?: number; height?: number; reason?: string };
  exit?: number;
  durationMs?: number;
  cwd?: string;
  verify?: string;
  git?: ProofGit;
}

export interface ChecklistItem {
  id?: string;
  text: string;
  built?: boolean;
  tested?: boolean;
  verify?: string;
  ui?: boolean;
  proof?: ChecklistProof;
}

export interface Vertebra {
  id: string;
  spineId: string;
  side?: "L" | "R";
  title: string;
  status?: string;
  files?: string[];
  resources?: string[];
  checklist?: ChecklistItem[];
}

export interface PlanSnapshot {
  title?: string;
  goal?: string;
  approved?: string;
  createdAt?: string;
  meta?: PlanMeta;
  spine: SpinePhase[];
  vertebrae: Vertebra[];
}

export interface EvidenceArtifact {
  name: string;
  mime: string;
  text?: string;
  dataUrl?: string;
}
