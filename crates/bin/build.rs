//! Embeds the icon and the version of `crest.exe` (assets/windows/crest.rc)
//! when building for Windows; nothing on other targets.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/windows/crest.rc");
    println!("cargo:rerun-if-changed=../../assets/windows/crest.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    // `1.2.3-rc.1` → `1,2,3,0`: four 16-bit numbers.
    let mut numbers: Vec<String> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse::<u16>().unwrap_or(0).to_string())
        .collect();
    numbers.resize(4, "0".to_owned());
    let macros = [
        format!("CREST_VERSION=\"{version}\""),
        format!("CREST_VERSION_NUMBERS={}", numbers.join(",")),
    ];
    if let Err(err) = embed_resource::compile_for(
        "../../assets/windows/crest.rc",
        ["crest"],
        // The icon is found next to the script.
        embed_resource::ParamsMacrosAndIncludeDirs(&macros, ["../../assets/windows"]),
    )
    .manifest_optional()
    {
        println!("cargo:warning=the Windows resources were not embedded: {err}");
    }
}
