# Design QA — Orchestrator messaging

- Source visual truth: `.claude/scratch/perfect-plan/evidence/PP-001-A04-before.png`
- Implementation screenshot: `artifacts/orchestrator-messaging-after.png`
- Combined comparison: `artifacts/orchestrator-messaging-design-qa-composite.png`
- Browser: installed Google Chrome, isolated headless Playwright context
- CSS viewport: 1440 × 900
- Source pixels: 4320 × 2700 at 3× density
- Implementation pixels: 2880 × 1800 at 2× density
- Normalization: source downsampled to 2880 × 1800; normalized images placed side by side without cropping
- State: existing repository/plan shell before the messenger versus the same shell class with the worker-note modal open
- Console: refreshed capture completed with zero console errors

## Findings

No actionable P0, P1 or P2 design differences remain.

The comparison is an intentional product extension rather than a pixel clone: the pre-change source has no messaging surface. The implementation preserves the existing cream, charcoal and green palette, monospaced typography, small-control bevels and high-density board layout while adding a distinct wide right-side chrome surface.

## Required fidelity surfaces

- Fonts and typography: existing monospace family, hierarchy, weights, line height and dense small-label treatment are preserved. Long message copy wraps without clipping.
- Spacing and layout rhythm: the dialog uses a stable wide column, aligned cards, visible scroll space and a separate composer footer. The underlying board remains legible and spatially unchanged.
- Colors and tokens: the new surface reuses existing cream, charcoal, muted green, red and amber semantic tokens. `UNROUTED` is visually distinct from `DELIVERED` and `ACKNOWLEDGED`.
- Image quality and assets: both captures are high-density PNGs. This UI contains no new raster asset or substituted logo/icon.
- Copy and content: the interface says `UNROUTED` when no connector exists and never displays `SENT` without durable delivery proof. Repository, worker and node identifiers are visible.

## Interaction evidence

The headless Chrome run exercised opening and closing the worker surface, exact worker/node targeting, note submission, unrouted escalation, local delivery, external worker-note ingestion, acknowledgement and switching to a second repository with the same plan number. Repository Alpha messages were absent from Repository Beta.

## Focused comparison

The right-side messenger occupies the focused implementation region at full 2880 × 1800 resolution. Its message states, worker/node heading, close control, note body, timestamps and composer are readable without crop or density extrapolation, so no additional crop was required.

## Comparison history

- Initial implementation review: no P0/P1/P2 visual mismatch found.
- No visual fix loop was required.
- Mechanical correction made before capture: the test was pinned to installed Google Chrome in headless mode, matching the selected browser.

## Follow-up polish

No blocking polish remains. A future optional pass could reduce the visual weight of long message bodies when a worker accumulates dozens of notes.

final result: passed

