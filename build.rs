fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = "core-interfaces/proto";
    println!("cargo:rerun-if-changed={}", proto_root);

    tonic_build::configure()
        // Bu bir sunucu olduğu için build_server(true) olmalı (varsayılan)
        .build_server(true)
        .build_client(true)
        .compile(
            &[format!("{}/sentiric/media/v1/media.proto", proto_root)],
            &[proto_root],
        )?;

    Ok(())
}