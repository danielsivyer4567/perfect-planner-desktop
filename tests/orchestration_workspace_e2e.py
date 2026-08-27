"""Focused contract proof for the compact, repository-scoped lifecycle projection."""

from __future__ import annotations

import os

from playwright.sync_api import sync_playwright


APP_URL = os.environ.get("PP_APP_URL", "http://127.0.0.1:5180/")


def main() -> None:
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        page = browser.new_page()
        page.goto(APP_URL)
        result = page.evaluate(
            """
            async () => {
              const workspace = await import('/src/services/orchestrationWorkspace.ts');
              const pipelineApi = await import('/src/services/orchestratorPipeline.ts');
              const supervisorApi = await import('/src/services/sessionSupervisor.ts');
              const board = {
                port: 5233, url: 'http://127.0.0.1:5233/',
                planPath: 'C:/repos/perfect/.claude/scratch/perfect-plan/plan.json',
                number: 'PP-001', topic: 'Reliable lifecycle', approved: 'yes', awaiting: null,
                pid: 10, project: 'Perfect Planner', repoName: 'Perfect Planner',
                repoRoot: 'C:/repos/perfect', worktreeName: 'perfect', branch: 'feature/reliable'
              };
              const node = (status) => ({
                id: 'A01', wave: 0, dependsOn: [], status, attempts: 1,
                lease: status === 'RUNNING' ? { workerId: 'worker-a' } : null
              });
              const pipeline = (status, reconciliation = null) => ({
                nowMs: Date.now(), preflightFresh: true,
                run: { status, organizationId: 'org', repositoryId: 'repo-perfect',
                  repositoryRoot: board.repoRoot, worktreePath: board.repoRoot,
                  branch: board.branch, runId: 'run-1', planId: 'PP-001', title: board.topic,
                  completedNodes: status === 'completed' ? 1 : 0, totalNodes: 1, updatedAt: new Date().toISOString() },
                scheduler: { nodes: { A01: node(status === 'completed' ? 'DONE' : 'RUNNING') }, completions: {} },
                stages: [], preflight: { disposition: 'ready' }, runApproval: {}, reconciliation,
                changes: [], release: null, delivery: null, leftovers: [], warnings: [], events: [],
                activeRuns: [], completedRuns: []
              });
              const messages = (repositoryRoot, planPath, state = 'delivered') => ({
                repositoryId: 'repo-perfect', organizationId: 'org', nowMs: Date.now(), registrations: [],
                stateCounts: { unrouted: 0, queued: 0, claimed: 0, delivered: state === 'delivered' ? 1 : 0,
                  acknowledged: 0, deadLetter: state === 'deadLetter' ? 1 : 0 },
                pendingAcknowledgementCount: 0, failedAttemptCount: 0, nextRetryAtMs: null,
                lastUpdatedAtMs: Date.now(), messages: [{ id: 'm1', authorId: 'worker-a', body: 'Tests passed.',
                  updatedAtMs: Date.now(), createdAtMs: Date.now(), scope: { repositoryRoot, planPath } }]
              });
              const derive = (overrides = {}) => workspace.deriveWorkspaceStatus({
                board, pipeline: null, controlPlane: null, workers: [], decisionCount: 0,
                identityError: null, supervisorError: null, ...overrides
              });
              const running = derive({ pipeline: pipeline('running') });
              const interrupted = derive({ workers: [{ state: 'GONE' }] });
              const recoveryBlocked = derive({ recoveryDeliveryError: 'A01: identity blocked' });
              const incomplete = derive({ pipeline: pipeline('completed') });
              const ready = derive({ pipeline: pipeline('completed', { passed: true }) });
              const deadLetter = derive({ controlPlane: messages(board.repoRoot, board.planPath, 'deadLetter') });
              const foreignMessage = derive({ controlPlane: messages('C:/repos/other', board.planPath) });
              const routed = derive({ controlPlane: messages(board.repoRoot, board.planPath) });
              const ports = pipelineApi.declaredRequiredPorts({ allowedResources: ['runtime:worker', 'port:8770', 'port:5233', 'port:5233'] });
              let malformedPortRejected = false;
              try { pipelineApi.declaredRequiredPorts({ allowedResources: ['port:unknown'] }); }
              catch { malformedPortRejected = true; }
              const event = { id: 'pp-reaper-1', kind: 'SESSION_CLEARED', atMs: 1,
                organizationId: 'org', planPath: board.planPath, vertebra: 'A01',
                sessionId: 'worker-a', fence: 2, reason: 'stale', files: [], resources: [] };
              const recoveryCases = {
                deliver: supervisorApi.classifyRecoveryMirror(event, { vertebrae: [{
                  id: 'A01', status: 'in-progress', startedBy: { session: 'worker-a' }
                }] }),
                alreadyApplied: supervisorApi.classifyRecoveryMirror(event, { vertebrae: [{
                  id: 'A01', status: 'recovery', recovery: { eventId: 'pp-reaper-1' }
                }] }),
                superseded: supervisorApi.classifyRecoveryMirror(event, { vertebrae: [{
                  id: 'A01', status: 'done', recovery: { eventId: 'pp-reaper-1' }
                }] }),
                blocked: supervisorApi.classifyRecoveryMirror(event, { vertebrae: [{
                  id: 'A01', status: 'in-progress', startedBy: { session: 'worker-b' }
                }] }),
                unverified: supervisorApi.classifyRecoveryMirror(event, null),
                selectedScope: supervisorApi.recoveryMirrorMatchesSelection(
                  event, board.repoRoot, { repositoryRoot: board.repoRoot, planPath: board.planPath }
                ),
                foreignRepository: supervisorApi.recoveryMirrorMatchesSelection(
                  event, 'C:/repos/foreign', { repositoryRoot: board.repoRoot, planPath: board.planPath }
                ),
                foreignPlan: supervisorApi.recoveryMirrorMatchesSelection(
                  event, board.repoRoot, { repositoryRoot: board.repoRoot, planPath: 'C:/repos/perfect/other.json' }
                ),
                noSelection: supervisorApi.recoveryMirrorMatchesSelection(
                  event, board.repoRoot, null
                ),
              };
              return { running, interrupted, recoveryBlocked, incomplete, ready, deadLetter, foreignMessage, routed,
                ports, malformedPortRejected, recoveryCases };
            }
            """
        )
        assert result["running"]["tone"] == "active"
        assert result["interrupted"]["tone"] == "blocked"
        assert "interrupted" in result["interrupted"]["nextAction"]
        assert result["recoveryBlocked"]["healthLabel"] == "Recovery delivery blocked"
        assert "Perfect Planner / PP-001" in result["recoveryBlocked"]["nextAction"]
        assert result["incomplete"]["ciLabel"] == "CI not ready"
        assert result["ready"]["ciLabel"] == "Ready for CI"
        assert result["deadLetter"]["tone"] == "blocked"
        assert "dead-letter" in result["deadLetter"]["nextAction"]
        assert result["foreignMessage"]["latestActivity"].startswith("No messages")
        assert result["routed"]["latestActivity"] == "worker-a: Tests passed."
        assert result["ports"] == [5233, 8770]
        assert result["malformedPortRejected"] is True
        assert result["recoveryCases"] == {
            "deliver": "DELIVER",
            "alreadyApplied": "ALREADY_APPLIED",
            "superseded": "SUPERSEDED",
            "blocked": "IDENTITY_BLOCKED",
            "unverified": "UNVERIFIED",
            "selectedScope": True,
            "foreignRepository": False,
            "foreignPlan": False,
            "noSelection": False,
        }
        browser.close()

    print("orchestration_workspace_e2e: PASS")
    print("proved: repository fence, interrupted state, routing/dead-letter action, lifecycle, CI readiness and restart recovery classification")


if __name__ == "__main__":
    main()
