fn main() {
    tauri_build::build();
    embed_test_manifest();
}

/// The unit-test binary links the same Tauri / WebView2 code as the app, which
/// imports `TaskDialogIndirect`. That function only exists in Common Controls v6, which
/// Windows hands out to executables whose manifest asks for it. `tauri_build` embeds the
/// manifest in the app binary only. Cargo cannot scope link arguments to library
/// unit tests, so opt in with ZUKO_TEST_MANIFEST=1 for `cargo test --lib` only.
/// Normal app builds must not link this duplicate resource. Without the
/// extra manifest the test executable dies at start-up
/// with STATUS_ENTRYPOINT_NOT_FOUND before running a single test.
fn embed_test_manifest() {
    use std::path::PathBuf;
    println!("cargo:rerun-if-env-changed=ZUKO_TEST_MANIFEST");
    if std::env::var("ZUKO_TEST_MANIFEST").as_deref() != Ok("1") {
        return;
    }
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
