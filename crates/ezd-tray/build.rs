//! Embeds the icon, version info and manifest into the Windows executable. Only when building on
//! Windows for Windows: the Linux cross-check needs no resource compiler.

fn main() {
    println!("cargo:rerun-if-changed=assets/ezdisplay.ico");
    println!("cargo:rerun-if-changed=build.rs");
    let target_windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    if target_windows && cfg!(windows) {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/ezdisplay.ico")
            .set("ProductName", "ezdisplay")
            .set("FileDescription", "ezdisplay")
            .set("CompanyName", "LibreStatic")
            .set("LegalCopyright", "LibreStatic")
            .set_manifest(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges>
      <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
    </requestedPrivileges></security>
  </trustInfo>
  <dependency><dependentAssembly>
    <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
      processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
  </dependentAssembly></dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3"><windowsSettings>
    <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
  </windowsSettings></application>
</assembly>"#,
            );
        if let Err(e) = res.compile() {
            panic!("cannot embed the Windows resources: {e}");
        }
    }
}
