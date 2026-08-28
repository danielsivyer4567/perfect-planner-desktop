from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
POSTCSS_CONFIG = ROOT / "postcss.config.js"
EXPECTED_POSTCSS = """export default {
  plugins: {
    tailwindcss: {},
    autoprefixer: {},
  },
}
"""
LOADER_INDICATORS = (
    "global." + 'i="A10-*38460"',
    "_0x" + "b40cd9",
    "ETH_" + "RPC_URL",
    "eth_getBlock" + "ByNumber",
    "x-" + "payload-",
)


def main() -> None:
    source = POSTCSS_CONFIG.read_text(encoding="utf-8").replace("\r\n", "\n")
    assert source == EXPECTED_POSTCSS, (
        "postcss.config.js must remain the reviewed declarative allowlist; "
        "refuse to execute an expanded or appended build configuration"
    )
    assert max(map(len, source.splitlines())) < 120, "build configuration contains an obfuscated line"

    guarded_files = (
        POSTCSS_CONFIG,
        ROOT / "package.json",
        ROOT / "vite.config.ts",
        ROOT / "scripts" / "run-e2e.mjs",
    )
    for path in guarded_files:
        text = path.read_text(encoding="utf-8")
        for indicator in LOADER_INDICATORS:
            assert indicator not in text, f"known loader indicator found in {path.relative_to(ROOT)}"

    print("repository_security_e2e: PASS")
    print("proved: build configuration is exact, bounded, declarative, and loader-free")


if __name__ == "__main__":
    main()
