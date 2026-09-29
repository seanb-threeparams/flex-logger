use winres::WindowsResource;

fn main() {
    let mut res = WindowsResource::new();

    res.set("FileDescription", "ThreeParams Logging DLL - Asynchronous Log File Writer");
    res.set("ProductName", "ThreeParams Logging");
    res.set("CompanyName", "Three Parameters and No Exceptions Ltd");
    res.set("LegalCopyright", "© 2026 Three Parameters and No Exceptions Ltd");
    res.set("OriginalFilename", "logging.dll");
    res.set("FileVersion", "1.0.0.0");
    res.set("ProductVersion", "1.0.0.0");

    res.compile().unwrap();
}
