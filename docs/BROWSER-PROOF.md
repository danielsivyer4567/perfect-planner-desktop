# Browser proof routing

Perfect Planner supports two explicit browser-control paths. They are complementary and must
never be reported as the same evidence source.

1. **Chrome MCP (preferred interactive controller).** The host agent discovers the latest
   connected Chrome MCP at runtime, drives the visible page, and attaches its own screenshots
   and logs. Availability is never inferred by the application or by a model.
2. **Playwright script (deterministic fallback).** `npm run proof:browser -- ...` launches
   installed Chrome when available and otherwise uses Playwright Chromium. It is cross-platform,
   headless, usable in Linux CI, and does not require model vision.

The fallback captures a full-page PNG, every console event, uncaught page errors, failed
requests, HTTP error responses, executed interaction steps, the selected browser engine, and a
JSON verdict. It exits non-zero for a missing expectation, browser/page error, unexpected failed
request, console error, HTTP navigation error, or missing screenshot. Requests cancelled by an
intentional page transition remain visible as `abortedRequests` but are not mislabelled as a
network defect.

Example:

```text
npm run proof:browser -- --url http://127.0.0.1:5180/ --name snapshots --click "#pp-btn-toggle-ui-navigation-map" --expect "#pp-region-ui-navigation-map" --expand-scroll ".stage-workspace.mapping"
```

The report always identifies `playwright-script` as its controller and sets
`chromeMcpClaimed: false`. A Chrome MCP run must be recorded by the host that actually invoked
MCP; the fallback cannot promote itself to MCP evidence.
