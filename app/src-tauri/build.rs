fn main() {
    tauri_build::build();
    embed_test_manifest();
}

/// The unit-test binary links the same Tauri / WebView2 code as the app, which
/// imports `TaskDialogIndirect`. That function only exists in Common Controls v6, which
/// Windows hands out to executables whose manifest asks for it. `tauri_build` embeds the
/// manifest in the app binary only (cargo has no link-arg scope for a lib's unit tests, so
/// the manifest goes to every executable; the app binary already carries two copies, the
/// linker keeps the first), so without this the test executable dies at start-up
/// with STATUS_ENTRYPOINT_NOT_FOUND before running a single test.
fn embed_test_manifest() {
    use std::path::PathBuf;
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest = out.join("zuko-test.manifest");
    std::fs::write(
        &manifest,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#,
    )
    .expect("write test manifest");
    let rc = out.join("zuko-test.rc");
    let manifest_path = manifest.to_string_lossy().replace('\\', "/");
    std::fs::write(&rc, format!("1 24 \"{manifest_path}\"\n")).expect("write test rc");
    embed_resource::compile_for_everything(rc, embed_resource::NONE)
        .manifest_optional()
        .expect("embed test manifest");
}
