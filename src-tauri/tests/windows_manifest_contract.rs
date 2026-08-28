const BUILD_SCRIPT: &str = include_str!("../build.rs");
const WINDOWS_MANIFEST: &str = include_str!("../windows-app.manifest.xml");

#[test]
fn build_script_embeds_the_reviewed_windows_manifest() {
    assert!(BUILD_SCRIPT.contains("WindowsAttributes::new()"));
    assert!(BUILD_SCRIPT.contains("windows_attributes(windows)"));
    assert!(BUILD_SCRIPT.contains("include_str!(\"windows-app.manifest.xml\")"));
}

#[test]
fn windows_manifest_keeps_common_controls_and_declares_per_monitor_v2() {
    assert!(WINDOWS_MANIFEST.contains("Microsoft.Windows.Common-Controls"));
    assert!(WINDOWS_MANIFEST.contains(">true/PM</dpiAware>"));
    assert!(WINDOWS_MANIFEST.contains(">PerMonitorV2, PerMonitor</dpiAwareness>"));
    assert!(!WINDOWS_MANIFEST.contains("requireAdministrator"));
}
