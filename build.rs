// build.rs — embed app icons and manifest into the .exe
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();

        // Primary icon (also used as the default APP_ICON resource by set_icon)
        res.set_icon_with_id("assets/app-idle.ico", "APP_ICON_IDLE");
        res.set_icon_with_id("assets/syncing.ico", "APP_ICON_SYNCING");
        res.set_icon_with_id("assets/complete.ico", "APP_ICON_COMPLETE");
        res.set_icon_with_id("assets/syncing1.ico", "APP_ICON_SYNC_1");
        res.set_icon_with_id("assets/syncing2.ico", "APP_ICON_SYNC_2");
        res.set_icon_with_id("assets/syncing3.ico", "APP_ICON_SYNC_3");
        res.set_icon_with_id("assets/syncing4.ico", "APP_ICON_SYNC_4");
        res.set_icon_with_id("assets/syncing 5.ico", "APP_ICON_SYNC_5");
        res.set_icon_with_id("assets/syncing 6.ico", "APP_ICON_SYNC_6");
        // set_icon sets the first ICON resource (ID 1), which Windows uses for
        // the file/taskbar thumbnail automatically.
        res.set_icon("assets/app-idle.ico");

        // Application manifest (enables visual styles + DPI awareness)
        res.set_manifest(r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="1.0.0.0" name="backupsynctool"/>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0" processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#);
        if let Err(e) = res.compile() {
            eprintln!("winres warning: {e}");
        }
    }
}
