const PRODUCT_NAME: &str = "QR Hub";
const FILE_DESCRIPTION: &str = "QR Hub - PostalAnnex+ Amazon QR Code Scanner";
const ORIGINAL_FILENAME: &str = "QR Hub.exe";
const COMPANY_NAME: &str = "PostalAnnex+";

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(target_os = "windows")]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/icon.ico");
            res.set("ProductName", PRODUCT_NAME);
            res.set("FileDescription", FILE_DESCRIPTION);
            res.set("OriginalFilename", ORIGINAL_FILENAME);
            res.set("CompanyName", COMPANY_NAME);
            if let Err(err) = res.compile() {
                panic!("Failed to compile Windows resource: {err}");
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
            && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
        {
            compile_cross_windows_resource();
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn compile_cross_windows_resource() {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    let Some(llvm_rc) = find_on_path("llvm-rc") else {
        println!(
            "cargo:warning=llvm-rc not found on PATH; skipping Windows .ico/VERSIONINFO resource embedding"
        );
        return;
    };

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let icon_path = manifest_dir.join("assets").join("icon.ico");
    let escaped_icon = escape_rc_path(&icon_path);

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
    let major: u16 = std::env::var("CARGO_PKG_VERSION_MAJOR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let minor: u16 = std::env::var("CARGO_PKG_VERSION_MINOR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let patch: u16 = std::env::var("CARGO_PKG_VERSION_PATCH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let rc_contents = format!(
        r#"1 ICON "{escaped_icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3f
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "{COMPANY_NAME}\0"
      VALUE "FileDescription", "{FILE_DESCRIPTION}\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "OriginalFilename", "{ORIGINAL_FILENAME}\0"
      VALUE "ProductName", "{PRODUCT_NAME}\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#
    );

    let rc_path = out_dir.join("app.rc");
    let res_path = out_dir.join("app.res");
    std::fs::write(&rc_path, rc_contents).expect("failed to write Windows .rc file");

    let status = Command::new(&llvm_rc)
        .current_dir(&out_dir)
        .arg("/no-preprocess")
        .arg("/FO")
        .arg("app.res")
        .arg("--")
        .arg("app.rc")
        .status()
        .expect("failed to execute llvm-rc");
    assert!(
        status.success(),
        "llvm-rc failed with status: {status}"
    );

    // Link only into binary targets, not unit test harnesses or proc-macros.
    println!("cargo:rustc-link-arg-bins={}", res_path.display());

    fn escape_rc_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn find_on_path(bin: &str) -> Option<PathBuf> {
        let path_var = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(bin);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}
