use anyhow::Result;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::ObjectCannedAcl;
use aws_sdk_s3::Client;
use bytes::Bytes;
use sha2::{Digest, Sha256};

use crate::config::AppConfig;

#[derive(Clone)]
pub struct StorageClient {
    client: Client,
    bucket: String,
}

impl StorageClient {
    pub async fn new(config: &AppConfig) -> Self {
        let creds = aws_credential_types::Credentials::new(
            &config.aws_access_key_id,
            &config.aws_secret_access_key,
            None,
            None,
            "env",
        );

        let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .credentials_provider(creds)
            .region(aws_sdk_s3::config::Region::new(config.aws_region.clone()))
            .endpoint_url(&config.aws_endpoint_url_s3)
            .load()
            .await;

        let client = Client::new(&aws_config);

        Self {
            client,
            bucket: config.bucket_name.clone(),
        }
    }

    pub async fn upload_episode_audio(
        &self,
        episode_id: &str,
        audio_bytes: Bytes,
    ) -> Result<String> {
        let hash = hex::encode(&Sha256::digest(&audio_bytes)[..8]);
        let key = format!("episodes/{}/{}.mp3", episode_id, hash);

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(audio_bytes))
            .content_type("audio/mpeg")
            .cache_control("public, max-age=31536000, immutable")
            .acl(ObjectCannedAcl::PublicRead)
            .send()
            .await?;

        Ok(format!(
            "https://{}.t3.storage.dev/{}",
            self.bucket, key
        ))
    }

    pub async fn upload_episode_image(
        &self,
        episode_id: &str,
        image_bytes: Bytes,
    ) -> Result<String> {
        let key = format!("episodes/{}/cover.jpg", episode_id);

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(image_bytes))
            .content_type("image/jpeg")
            .cache_control("public, max-age=31536000, immutable")
            .acl(ObjectCannedAcl::PublicRead)
            .send()
            .await?;

        Ok(format!(
            "https://{}.t3.storage.dev/{}",
            self.bucket, key
        ))
    }

    pub async fn delete_object(&self, url: &str) -> Result<()> {
        let key = url
            .strip_prefix(&format!("https://{}.t3.storage.dev/", self.bucket))
            .or_else(|| {
                url.strip_prefix(&format!(
                    "https://{}.fly.storage.tigris.dev/",
                    self.bucket
                ))
            })
            .unwrap_or(url);
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await?;
        Ok(())
    }
}
