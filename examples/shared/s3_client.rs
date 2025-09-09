use anyhow::Result;
use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use std::env;

pub async fn connect_to_s3() -> Result<S3Client> {
    let access_key_id = env::var("S3_ACCESS_KEY_ID")?;
    let secret_access_key = env::var("S3_SECRET_ACCESS_KEY")?;
    let endpoint_url = env::var("S3_ENDPOINT_URL")?;
    let region = env::var("S3_REGION")?;
    
    let credentials_provider = aws_credential_types::Credentials::new(
        access_key_id, secret_access_key, None, None, "Static"
    );
    
    let config = aws_config::defaults(BehaviorVersion::latest())
        .endpoint_url(endpoint_url)
        .region(aws_config::Region::new(region))
        .credentials_provider(credentials_provider)
        .load()
        .await;
        
    // --- DEĞİŞİKLİK BURADA ---
    // test-runner'ın da path-style erişimi kullanmasını zorunlu kıl.
    let s3_config = aws_sdk_s3::config::Builder::from(&config)
        .force_path_style(true) 
        .build();
    // -------------------------
        
    Ok(S3Client::from_conf(s3_config))
}