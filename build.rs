fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("cupertino".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint build failed");

    // Windows: give nava.exe its icon and version info.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("packaging/windows/nava.ico");
        res.set("ProductName", "Nava");
        res.set("FileDescription", "Nava");
        res.compile().expect("Windows resources failed");
    }
}
