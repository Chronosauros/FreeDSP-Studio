// The Windows exe carries the app icon and version details. Nothing to do elsewhere.
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/icon.ico");
        res.set("ProductName", "FreeDSP Studio");
        res.set("FileDescription", "FreeDSP Studio");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded: {}", e);
        }
    }
    println!("cargo:rerun-if-changed=assets/icons/icon.ico");
}
