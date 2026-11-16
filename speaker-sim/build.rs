//! Icône de l'exécutable Windows (ressource .ico) ; rien à faire ailleurs.

fn main() {
    println!("cargo:rerun-if-changed=packaging/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("packaging/icon.ico");
        res.set("ProductName", "Speaker Sim");
        res.set("FileDescription", "Speaker Sim (devfrp)");
        if let Err(e) = res.compile() {
            println!("cargo:warning=ressources Windows non compilées : {e}");
        }
    }
}
