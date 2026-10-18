// Exe-Icon und Versionsinfo für Windows einbetten.
fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icons/icon.ico")
            .set("ProductName", "MyLauncher")
            .set("FileDescription", "MyLauncher")
            .set("CompanyName", "Stephanides IT e.U.")
            .set("LegalCopyright", "Stephanides IT e.U.");
        res.compile().expect("Windows-Ressourcen");
    }
}
