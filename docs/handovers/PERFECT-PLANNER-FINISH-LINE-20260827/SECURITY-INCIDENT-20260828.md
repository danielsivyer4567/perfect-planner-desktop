# Build-configuration security incident — 2026-08-28

## Finding

Remote commit `1f3a18337639d1d76bd75164d4805fd40db3a4c2` appended an obfuscated,
network-capable Node loader to `postcss.config.js`. The payload referenced HTTP clients,
Ethereum RPC discovery, dynamic evaluation, and detached `node -e` execution. It was unrelated to
the stated formatting/ignore change and is treated as malicious.

The real integration stopped before any merged build command ran. The final tree in merge commit
`4a4d50ecc8589a574453bb6f22e5402aea50cd28` restores the reviewed declarative PostCSS file and adds
a fail-closed build-configuration gate.

## Exposure boundary

An earlier Git-less integration simulation did run `npm run build` before the appended payload was
recognized, so arbitrary code may have executed under the current Windows account. No claim is made
that a process/network check or antivirus scan is equivalent to a forensic clean-room assessment.

## Containment and verification performed

- Restored the exact six-line declarative PostCSS configuration; SHA-256
  `190C877DB466995BF1482F4A16ABD06E04A89EDE3119341E2A86FF96E1737B27`.
- Added `tests/repository_security_e2e.py`, enforced before dev, host-dev, build, browser tests,
  preview, every Tauri command, and as an explicit hosted-CI step.
- Replaced ignored JavaScript dependencies with `npm ci --ignore-scripts`; 109 packages installed,
  package audit reported zero vulnerabilities.
- Found no live detached `node -e` loader, known loader command indicator, or non-loopback TCP
  connection owned by Node.
- Microsoft Defender real-time and behavior monitoring were enabled. A fresh quick scan ran from
  14:18:39 to 14:27:36 local time and produced zero new detections. Defender had also completed a
  quick scan at 09:30 and a full scan at 11:15 on the same day.
- Scanned the final working tree, excluding retained audit evidence and generated dependencies, for
  the known loader indicators; none remained.
- Re-ran frontend build, all eight browser suites, Rust format, 309 active Rust tests plus 17
  contract tests, warning-denied Clippy, Windows MSI/NSIS packaging, and packaged/installed native
  smoke after containment.

## Remaining security actions

1. Treat credentials available to this Windows account at exposure time as potentially disclosed;
   rotate them from a known-clean device, starting with source-control, package-registry, cloud,
   signing, and deployment credentials.
2. Perform the production build and signing on a known-clean Windows host or freshly provisioned VM.
3. Do not promote artifacts from this machine. The locally built MSI/NSIS files are unsigned test
   candidates only.
4. Keep the final-tree security gate in CI. The compromised commit remains in remote history as the
   current `origin/main`; merging the sanitized feature tip creates a clean final tree but does not
   erase historical exposure.
