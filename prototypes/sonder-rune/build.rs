//! Embeds Sonder's icon in the Windows executable, as the main app does.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile(
            "app.rc",
            embed_resource::ParamsIncludeDirs(["../../assets"]),
        )
        .manifest_required()
        .expect("could not embed the Windows icon");
    }
}
