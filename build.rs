use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Cargo, git bağımlılıklarını %USERPROFILE%/.cargo/git/checkouts altında bir yere klonlar.
    // Bu ortam değişkeni, 'sentiric-contracts' paketinin kaynak kodunun nerede olduğunu bize söyler.
    let contracts_dir = env::var("CARGO_MANIFEST_DIR_sentiric-contracts")
        .expect("sentiric-contracts dependency not found. Did you run 'cargo build'?");

    let proto_path = PathBuf::from(contracts_dir).join("proto");

    println!("cargo:rerun-if-changed={}", proto_path.display());

    tonic_build::configure()
        .build_server(true)
        .build_client(true) // İleride başka servislere istek atması gerekebilir diye açık bırakalım.
        .compile(
            &[
                proto_path.join("sentiric/media/v1/media.proto"),
                // İleride gerekebilecek diğer protoları buraya ekleyebiliriz.
            ],
            &[proto_path], // Import'ların aranacağı kök dizin
        )?;

    Ok(())
}