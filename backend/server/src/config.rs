use std::env;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub database_url: String,
    pub aws_access_key_id: String,
    pub aws_secret_access_key: String,
    pub aws_endpoint_url_s3: String,
    pub aws_region: String,
    pub bucket_name: String,
    pub anthropic_api_key: String,
    pub google_tts_api_key: String,
    pub google_studio_api_key: String,
    pub admin_token: String,
    pub google_tts_voice: String,
    pub host: String,
    pub port: u16,
    pub worker_poll_interval: u64,
    pub worker_count: usize,
    pub max_job_attempts: i32,
    pub public_url: String,
    pub generate_images: bool,
    /// Provider for clean/summarize: "claude" or "gemini"
    pub ai_provider: String,
    /// PDF extractor: "claude" or "gemini"
    pub pdf_extractor: String,
}

impl AppConfig {
    pub fn make_provider(&self) -> tts_lib::Provider {
        match self.ai_provider.as_str() {
            "gemini" => tts_lib::Provider::gemini_default(self.google_studio_api_key.clone()),
            _ => tts_lib::Provider::claude(self.anthropic_api_key.clone()),
        }
    }
}

impl AppConfig {
    pub fn from_env() -> Self {
        let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
        let port = env::var("PORT")
            .unwrap_or_else(|_| "8080".into())
            .parse()
            .expect("PORT must be a valid u16");

        let public_url =
            env::var("PUBLIC_URL").unwrap_or_else(|_| format!("http://{}:{}", host, port));

        Self {
            database_url: required("DATABASE_URL"),
            aws_access_key_id: required("AWS_ACCESS_KEY_ID"),
            aws_secret_access_key: required("AWS_SECRET_ACCESS_KEY"),
            aws_endpoint_url_s3: required("AWS_ENDPOINT_URL_S3"),
            aws_region: env::var("AWS_REGION").unwrap_or_else(|_| "auto".into()),
            bucket_name: required("BUCKET_NAME"),
            anthropic_api_key: required("ANTHROPIC_API_KEY"),
            google_tts_api_key: required("GOOGLE_TTS_API_KEY"),
            google_studio_api_key: required("GOOGLE_STUDIO_API_KEY"),
            admin_token: required("ADMIN_TOKEN"),
            google_tts_voice: env::var("GOOGLE_TTS_VOICE")
                .unwrap_or_else(|_| "en-US-Chirp3-HD-Puck".into()),
            host,
            port,
            worker_poll_interval: env::var("WORKER_POLL_INTERVAL_SECS")
                .unwrap_or_else(|_| "5".into())
                .parse()
                .expect("WORKER_POLL_INTERVAL_SECS must be a valid u64"),
            worker_count: env::var("WORKER_COUNT")
                .unwrap_or_else(|_| "2".into())
                .parse()
                .expect("WORKER_COUNT must be a valid usize"),
            max_job_attempts: env::var("MAX_JOB_ATTEMPTS")
                .unwrap_or_else(|_| "3".into())
                .parse()
                .expect("MAX_JOB_ATTEMPTS must be a valid i32"),
            public_url,
            generate_images: env::var("GENERATE_IMAGES")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true),
            ai_provider: env::var("AI_PROVIDER").unwrap_or_else(|_| "claude".into()),
            pdf_extractor: env::var("PDF_EXTRACTOR").unwrap_or_else(|_| "gemini".into()),
        }
    }
}

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} environment variable is required"))
}
