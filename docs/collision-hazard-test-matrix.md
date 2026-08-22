# Collision Assessor PowerShell hazard matrix

This is a pre-certification sentinel for the collision-assessor foundations that are already
built. It deliberately does **not** activate worker admission, the approval-to-chat bridge, or
the production scheduler authority. Those paths remain gated by PP-002 B15, B20, B09 and B10;
B12 must extend this matrix when they exist.

Run from the isolated Perfect Planner Desktop worktree:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/Invoke-CollisionHazardTests.ps1 -ValidateOnly
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/Invoke-CollisionHazardTests.ps1 -Profile Quick
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/Invoke-CollisionHazardTests.ps1 -Profile Full
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/Invoke-CollisionHazardTests.ps1 -Profile Stress -Stage Journal,Tickets,Scale -Repeat 20
```

The runner executes one exact allowlisted Cargo test at a time. Before every case it checks free
RAM and CPU load, waits briefly if the machine is busy, and aborts rather than adding uncontrolled
pressure. Every child test has a hard deadline. Logs and `summary.json` are written only below
`artifacts/hazard-tests/`; an outside path, existing output directory, or junction/reparse-point
component is rejected so one run cannot escape the repo or overwrite another run's evidence.

`Stress` runs the full selected stages once, then repeats only their explicitly designated
contention cases. It does not multiply every ordinary test by the repeat count.

The runner fingerprints `HEAD`, the complete tracked diff, and every untracked source file before
and after execution. Any mid-run source change is reported as `SOURCE_DRIFT` and cannot certify the
run.

| Stage | Boundary | Common hazards attacked |
|---|---|---|
| Registry | machine registry | incomplete coverage, malformed state, alias swaps, lost updates, post-scan plan races |
| Discovery | one-use capability | replay, nonce rebinding, truncated/oversized frames, unexpected plans |
| NativeCensus | native collection | concurrent replay, mid-call expiry/revoke, identity drift, error-detail leakage |
| CanonicalClaims | signed claim snapshot | glob ambiguity, unsigned mutation, subset laundering, issuer rotation |
| CollisionGraph | pure analyzer | discovery-order drift, false prefixes, duplicates, contradictory policy, overflow |
| Snapshot | immutable assessment | hidden conflicts, proof splicing/truncation, reordering, same-key byte replacement |
| Clearance | one-use admission permit | parallel replay, exact expiry, binding drift, invalid MAC, prior epoch |
| Tickets | owner mailbox | accidental production enablement, cross-owner access, route expiry, duplicate events |
| Journal | audit durability | concurrent writer loss, torn writes, corruption, rollback, unanchored restart |
| Scale | bounded contention | quadratic fanout, duplicate delivery, oversized assessment and ticket sets |

Exit codes are stable: `0` pass, `1` test failure, `2` validation/configuration failure, `3`
resource-pressure abort, `4` child-test timeout, and `5` source drift. A skipped or missing test is
never counted as a pass.
